//! Public identity of the platform: name, primary colour, logo, legal pages.
//!
//! Everything here lives in the `settings` table (non-secret keys) and is readable without an account
//! (the landing and sign-up pages need it). Writes are admin-only and audited. The settings *page* UI
//! arrives in phase 1-3; these endpoints are what it will call.

use crate::platform::{audit, bad, db_err, lock, require_admin, text, Res};
use crate::platform_settings as settings;
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const KEY_NAME: &str = "brand.name";
const KEY_COLOR: &str = "brand.color";
const KEY_LOGO_MIME: &str = "brand.logo_mime";
const KEY_LOGO_STAMP: &str = "brand.logo_stamp";
const LOGO_FILE: &str = "brand-logo";
const MAX_LOGO_BYTES: usize = 512 * 1024;
const MAX_LEGAL_CHARS: usize = 20_000;
/// WCAG AA for normal text: the primary colour carries white button labels.
const MIN_CONTRAST_ON_WHITE: f64 = 4.5;

// ───────── colour maths ─────────

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ])
}

fn luminance([r, g, b]: [u8; 3]) -> f64 {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Normalises to upper-case `#RRGGBB` and enforces readable white text on it.
fn valid_brand_color(raw: &str) -> Res<String> {
    let rgb = parse_hex(raw.trim()).ok_or_else(|| bad("invalid_color"))?;
    if contrast_ratio(rgb, [255, 255, 255]) < MIN_CONTRAST_ON_WHITE {
        return Err(bad("low_contrast"));
    }
    Ok(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
}

// ───────── public config ─────────

#[derive(Serialize, Debug, PartialEq)]
pub struct Legal {
    privacy: bool,
    terms: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PublicConfig {
    name: Option<String>,
    color: Option<String>,
    /// Relative URL with a cache-busting stamp, when a logo is set.
    logo: Option<String>,
    legal: Legal,
}

fn public_config(conn: &Connection, crypto: &settings::Crypto) -> Res<PublicConfig> {
    let get = |k: &str| settings::get(conn, crypto, k);
    let logo = match (get(KEY_LOGO_MIME)?, get(KEY_LOGO_STAMP)?) {
        (Some(_), Some(stamp)) => Some(format!("/api/platform/public/logo?v={stamp}")),
        _ => None,
    };
    Ok(PublicConfig {
        name: get(KEY_NAME)?,
        color: get(KEY_COLOR)?,
        logo,
        legal: Legal { privacy: settings::is_set(conn, "legal.privacy")?, terms: settings::is_set(conn, "legal.terms")? },
    })
}

pub async fn public_config_handler(State(s): State<Arc<AppState>>) -> Res<Response> {
    let cfg = public_config(&*lock(&s)?, &s.platform.crypto)?;
    Ok(([(header::CACHE_CONTROL, "no-cache")], Json(cfg)).into_response())
}

// ───────── branding (admin) ─────────

#[derive(Deserialize)]
pub struct BrandingReq {
    /// `Some("")` clears the value.
    name: Option<String>,
    color: Option<String>,
}

fn set_branding(conn: &Connection, crypto: &settings::Crypto, admin_id: &str, r: &BrandingReq) -> Res<()> {
    // Validate everything first so a bad colour does not leave a half-applied change.
    let name = match r.name.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(n) => Some(Some(text(n, 60, "invalid_name")?)),
        None => None,
    };
    let color = match r.color.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(c) => Some(Some(valid_brand_color(c)?)),
        None => None,
    };
    for (key, change) in [(KEY_NAME, name), (KEY_COLOR, color)] {
        match change {
            Some(Some(v)) => settings::set(conn, crypto, key, &v, false, Some(admin_id))?,
            Some(None) => settings::delete(conn, key)?,
            None => continue,
        }
        audit(conn, admin_id, key, "setting_changed", key);
    }
    Ok(())
}

pub async fn set_branding_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<BrandingReq>) -> Res<StatusCode> {
    let admin = require_admin(&s, &h)?;
    set_branding(&*lock(&s)?, &s.platform.crypto, &admin.id, &r)?;
    Ok(StatusCode::NO_CONTENT)
}

/// png / jpeg only (SVG can carry script). Returns the mime type.
fn validate_logo(bytes: &[u8], filename: &str) -> Res<&'static str> {
    if bytes.is_empty() || bytes.len() > MAX_LOGO_BYTES {
        return Err(bad("invalid_file_size"));
    }
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Ok("image/png"),
        "jpg" | "jpeg" if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) => Ok("image/jpeg"),
        "png" | "jpg" | "jpeg" => Err(bad("file_content_mismatch")),
        _ => Err(bad("file_type_not_allowed")),
    }
}

pub async fn upload_logo_handler(State(s): State<Arc<AppState>>, h: HeaderMap, mut mp: Multipart) -> Res<StatusCode> {
    let admin = require_admin(&s, &h)?;
    let mut upload = None;
    while let Some(field) = mp.next_field().await.map_err(|_| bad("invalid_upload"))? {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("").to_string();
            let bytes = field.bytes().await.map_err(|_| bad("invalid_file_size"))?;
            upload = Some((name, bytes));
            break;
        }
    }
    let (name, bytes) = upload.ok_or_else(|| bad("invalid_upload"))?;
    let mime = validate_logo(&bytes, &name)?;
    std::fs::write(s.platform.files_dir.join(LOGO_FILE), &bytes).map_err(db_err)?;
    let conn = lock(&s)?;
    settings::set(&conn, &s.platform.crypto, KEY_LOGO_MIME, mime, false, Some(&admin.id))?;
    settings::set(&conn, &s.platform.crypto, KEY_LOGO_STAMP, &now_ms().to_string(), false, Some(&admin.id))?;
    audit(&conn, &admin.id, KEY_LOGO_MIME, "setting_changed", "logo");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_logo_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<StatusCode> {
    let admin = require_admin(&s, &h)?;
    let conn = lock(&s)?;
    settings::delete(&conn, KEY_LOGO_MIME)?;
    settings::delete(&conn, KEY_LOGO_STAMP)?;
    let _ = std::fs::remove_file(s.platform.files_dir.join(LOGO_FILE));
    audit(&conn, &admin.id, KEY_LOGO_MIME, "setting_changed", "logo removed");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn logo_handler(State(s): State<Arc<AppState>>) -> Res<Response> {
    let mime = settings::get(&*lock(&s)?, &s.platform.crypto, KEY_LOGO_MIME)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let bytes = std::fs::read(s.platform.files_dir.join(LOGO_FILE)).map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        bytes,
    )
        .into_response())
}

pub fn logo_upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_LOGO_BYTES + 64 * 1024)
}

// ───────── legal pages ─────────

#[derive(Serialize, Debug)]
pub struct LegalPage {
    slug: String,
    /// Plain text, shown as-is (never rendered as HTML). `None` until the admin writes it.
    body: Option<String>,
}

fn legal_key(slug: &str) -> Res<String> {
    match slug {
        "privacy" | "terms" => Ok(format!("legal.{slug}")),
        _ => Err(err(StatusCode::NOT_FOUND, "not_found")),
    }
}

pub async fn legal_get_handler(State(s): State<Arc<AppState>>, Path(slug): Path<String>) -> Res<Json<LegalPage>> {
    let key = legal_key(&slug)?;
    let body = settings::get(&*lock(&s)?, &s.platform.crypto, &key)?;
    Ok(Json(LegalPage { slug, body }))
}

#[derive(Deserialize)]
pub struct LegalReq {
    body: String,
}

fn set_legal(conn: &Connection, crypto: &settings::Crypto, admin_id: &str, slug: &str, body: &str) -> Res<()> {
    let key = legal_key(slug)?;
    let body = body.trim();
    if body.chars().count() > MAX_LEGAL_CHARS {
        return Err(bad("legal_too_long"));
    }
    if body.is_empty() {
        settings::delete(conn, &key)?;
    } else {
        settings::set(conn, crypto, &key, body, false, Some(admin_id))?;
    }
    audit(conn, admin_id, &key, "setting_changed", &key);
    Ok(())
}

pub async fn legal_put_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(slug): Path<String>, Json(r): Json<LegalReq>) -> Res<StatusCode> {
    let admin = require_admin(&s, &h)?;
    set_legal(&*lock(&s)?, &s.platform.crypto, &admin.id, &slug, &r.body)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::create_test_db;

    fn cx() -> settings::Crypto {
        settings::Crypto::for_tests()
    }

    #[test]
    fn contrast_math_matches_wcag_reference_values() {
        let white = [255, 255, 255];
        assert!((contrast_ratio([0, 0, 0], white) - 21.0).abs() < 0.01, "black on white = 21:1");
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-9);
        assert!((contrast_ratio([119, 119, 119], white) - 4.48).abs() < 0.02, "#777 is the classic just-below-AA grey");
        assert!(contrast_ratio([118, 118, 118], white) >= 4.5, "#767676 is the lightest grey passing AA");
        assert_eq!(contrast_ratio([10, 20, 30], white), contrast_ratio(white, [10, 20, 30]), "symmetric");
    }

    #[test]
    fn brand_colour_is_validated_normalised_and_must_be_readable() {
        assert_eq!(valid_brand_color(" #1a6cff ").unwrap(), "#1A6CFF");
        assert_eq!(valid_brand_color("#0B5D3B").unwrap(), "#0B5D3B");
        for bad_hex in ["", "red", "#12345", "#1234567", "1A6CFF", "#GGGGGG", "#12 456", "#ffffff", "#ffeb3b", "#aaaaaa"] {
            let e = valid_brand_color(bad_hex).unwrap_err();
            assert_eq!(e.0, StatusCode::BAD_REQUEST, "{bad_hex}");
        }
        assert!(valid_brand_color("#ffeb3b").unwrap_err().1.contains("low_contrast"));
        assert!(valid_brand_color("#12345").unwrap_err().1.contains("invalid_color"));
        // multibyte input must not panic on slicing
        assert!(valid_brand_color("#ééé").is_err());
    }

    #[test]
    fn branding_roundtrip_clear_and_atomic_validation() {
        let conn = create_test_db();
        let c = cx();
        assert_eq!(public_config(&conn, &c).unwrap(), PublicConfig { name: None, color: None, logo: None, legal: Legal { privacy: false, terms: false } });
        set_branding(&conn, &c, "a1", &BrandingReq { name: Some("  أكاديمية النور  ".into()), color: Some("#0b5d3b".into()) }).unwrap();
        let cfg = public_config(&conn, &c).unwrap();
        assert_eq!((cfg.name.as_deref(), cfg.color.as_deref()), (Some("أكاديمية النور"), Some("#0B5D3B")));
        // a bad colour must not apply the (valid) name from the same request
        let err = set_branding(&conn, &c, "a1", &BrandingReq { name: Some("اسم جديد".into()), color: Some("#ffff00".into()) }).unwrap_err();
        assert!(err.1.contains("low_contrast"));
        assert_eq!(public_config(&conn, &c).unwrap().name.as_deref(), Some("أكاديمية النور"), "nothing half-applied");
        assert!(set_branding(&conn, &c, "a1", &BrandingReq { name: Some("x".repeat(61)), color: None }).is_err());
        // empty string clears
        set_branding(&conn, &c, "a1", &BrandingReq { name: Some("".into()), color: None }).unwrap();
        let cfg = public_config(&conn, &c).unwrap();
        assert_eq!((cfg.name, cfg.color.as_deref()), (None, Some("#0B5D3B")));
        let audited: i64 = conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'setting_changed'", [], |r| r.get(0)).unwrap();
        assert_eq!(audited, 3, "name set, colour set, name cleared");
    }

    #[test]
    fn logo_accepts_only_real_png_or_jpeg_within_limit() {
        let png = b"\x89PNG\r\n\x1a\nxxxx";
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
        assert_eq!(validate_logo(png, "logo.PNG").unwrap(), "image/png");
        assert_eq!(validate_logo(&jpg, "a.jpeg").unwrap(), "image/jpeg");
        assert!(validate_logo(png, "logo.jpg").unwrap_err().1.contains("file_content_mismatch"), "extension/content mismatch");
        assert!(validate_logo(b"<svg onload=alert(1)>", "logo.svg").unwrap_err().1.contains("file_type_not_allowed"));
        assert!(validate_logo(b"<svg onload=alert(1)>", "logo.png").unwrap_err().1.contains("file_content_mismatch"), "svg renamed to png");
        assert!(validate_logo(b"", "a.png").is_err());
        let mut big = png.to_vec();
        big.resize(MAX_LOGO_BYTES + 1, 0);
        assert!(validate_logo(&big, "a.png").unwrap_err().1.contains("invalid_file_size"));
        big.truncate(MAX_LOGO_BYTES);
        assert!(validate_logo(&big, "a.png").is_ok(), "exactly at the limit is fine");
        assert!(validate_logo(png, "noext").is_err());
    }

    #[test]
    fn legal_pages_only_two_slugs_limits_and_clear() {
        let conn = create_test_db();
        let c = cx();
        for bad_slug in ["", "cookies", "../privacy", "PRIVACY"] {
            assert_eq!(legal_key(bad_slug).unwrap_err().0, StatusCode::NOT_FOUND, "{bad_slug}");
        }
        set_legal(&conn, &c, "a1", "privacy", "  نص الخصوصية <script>alert(1)</script>  ").unwrap();
        assert_eq!(settings::get(&conn, &c, "legal.privacy").unwrap().as_deref(), Some("نص الخصوصية <script>alert(1)</script>"), "stored verbatim (rendered as text by the UI)");
        let cfg = public_config(&conn, &c).unwrap();
        assert_eq!((cfg.legal.privacy, cfg.legal.terms), (true, false));
        assert!(set_legal(&conn, &c, "a1", "terms", &"ن".repeat(MAX_LEGAL_CHARS + 1)).unwrap_err().1.contains("legal_too_long"));
        assert!(set_legal(&conn, &c, "a1", "terms", &"ن".repeat(MAX_LEGAL_CHARS)).is_ok(), "limit counts characters, not bytes");
        set_legal(&conn, &c, "a1", "privacy", "   ").unwrap();
        assert!(!public_config(&conn, &c).unwrap().legal.privacy, "blank body removes the page");
    }
}
