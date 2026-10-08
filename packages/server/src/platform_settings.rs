//! Encrypted platform settings.
//!
//! Secrets (AI key, SMTP password) are stored AES-256-GCM encrypted in the `settings` table.
//! The master key lives in a file next to the database (mode 0600), is generated on first run and is
//! part of backups. Each ciphertext is bound to its setting name (AAD), so a value copied from one
//! row to another fails to decrypt.
//!
//! The HTTP surface (settings page) arrives in phase 1-3; until then these are tested primitives.
#![allow(dead_code)]

use crate::platform::{bad, db_err, Res};
use crate::relay::now_ms;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  encrypted INTEGER NOT NULL DEFAULT 0,
  updated_by TEXT,
  updated_at INTEGER NOT NULL
);
";

const TOKEN_PREFIX: &str = "v1.";
const MAX_VALUE_CHARS: usize = 20_000;

pub struct Crypto {
    key: LessSafeKey,
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_decode_key(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Reads the master key, creating it (mode 0600, never overwriting) when the file does not exist.
fn load_or_create_key(path: &Path) -> Result<[u8; 32], String> {
    let parse = |text: String| {
        hex_decode_key(&text).ok_or_else(|| format!("{} is not a valid key file (expected 64 hex chars); refusing to overwrite it", path.display()))
    };
    match std::fs::read_to_string(path) {
        Ok(text) => return parse(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    }
    let mut key = [0u8; 32];
    SystemRandom::new().fill(&mut key).map_err(|_| "random source unavailable".to_string())?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(hex_encode(&key).as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Ok(key)
        }
        // Lost a creation race with another process: use the key it wrote.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => parse(std::fs::read_to_string(path).map_err(|e| e.to_string())?),
        Err(e) => Err(format!("cannot create {}: {e}", path.display())),
    }
}

impl Crypto {
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, String> {
        let unbound = UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| "invalid key".to_string())?;
        Ok(Self { key: LessSafeKey::new(unbound) })
    }

    pub fn from_key_file(path: &Path) -> Result<Self, String> {
        Self::from_bytes(&load_or_create_key(path)?)
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self::from_bytes(&[7u8; 32]).expect("test key")
    }

    /// `aad` (the setting name) is authenticated but not stored.
    pub fn encrypt(&self, aad: &str, plaintext: &str) -> Result<String, String> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| "random source unavailable".to_string())?;
        let mut buf = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), &mut buf)
            .map_err(|_| "encryption failed".to_string())?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&buf);
        Ok(format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(out)))
    }

    pub fn decrypt(&self, aad: &str, token: &str) -> Result<String, String> {
        let body = token.strip_prefix(TOKEN_PREFIX).ok_or("unknown ciphertext version")?;
        let raw = URL_SAFE_NO_PAD.decode(body).map_err(|_| "corrupt ciphertext")?;
        if raw.len() < NONCE_LEN + AES_256_GCM.tag_len() {
            return Err("corrupt ciphertext".into());
        }
        let (nonce, sealed) = raw.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| "corrupt ciphertext")?;
        let mut buf = sealed.to_vec();
        let plain = self
            .key
            .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), &mut buf)
            .map_err(|_| "decryption failed (wrong key, wrong setting, or tampered data)")?;
        String::from_utf8(plain.to_vec()).map_err(|_| "decrypted value is not UTF-8".to_string())
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

pub fn set(conn: &Connection, crypto: &Crypto, key: &str, value: &str, encrypted: bool, updated_by: Option<&str>) -> Res<()> {
    if !valid_key(key) || value.chars().count() > MAX_VALUE_CHARS {
        return Err(bad("invalid_setting"));
    }
    let stored = if encrypted { crypto.encrypt(key, value).map_err(db_err)? } else { value.to_string() };
    conn.execute(
        "INSERT INTO settings(key, value, encrypted, updated_by, updated_at) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, encrypted = excluded.encrypted,
           updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![key, stored, encrypted, updated_by, now_ms()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub fn get(conn: &Connection, crypto: &Crypto, key: &str) -> Res<Option<String>> {
    let row: Option<(String, bool)> = conn
        .query_row("SELECT value, encrypted FROM settings WHERE key = ?1", params![key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(db_err)?;
    match row {
        None => Ok(None),
        Some((v, false)) => Ok(Some(v)),
        Some((v, true)) => crypto.decrypt(key, &v).map(Some).map_err(db_err),
    }
}

/// True when a value exists (used by the UI to show "saved ✓" without ever returning a secret).
pub fn is_set(conn: &Connection, key: &str) -> Res<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)", params![key], |r| r.get(0)).map_err(db_err)
}

pub fn delete(conn: &Connection, key: &str) -> Res<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key]).map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::create_test_db;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("exameow-test-{}-{name}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn roundtrip_arabic_and_empty_and_unique_nonce() {
        let c = Crypto::for_tests();
        for text in ["sk-test-123", "مفتاح سري 🔑", "", &"x".repeat(5000)] {
            let t = c.encrypt("ai.key", text).unwrap();
            assert!(t.starts_with("v1.") && !t.contains(text) || text.is_empty());
            assert_eq!(c.decrypt("ai.key", &t).unwrap(), text);
        }
        assert_ne!(c.encrypt("k", "same").unwrap(), c.encrypt("k", "same").unwrap(), "random nonce per encryption");
    }

    #[test]
    fn tampering_wrong_key_wrong_name_and_garbage_are_rejected() {
        let c = Crypto::for_tests();
        let t = c.encrypt("ai.key", "secret").unwrap();
        // flip one character in the ciphertext body
        let mut chars: Vec<char> = t.chars().collect();
        let i = chars.len() - 3;
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        assert!(c.decrypt("ai.key", &chars.into_iter().collect::<String>()).is_err(), "tamper");
        assert!(Crypto::from_bytes(&[8u8; 32]).unwrap().decrypt("ai.key", &t).is_err(), "wrong key");
        assert!(c.decrypt("smtp.password", &t).is_err(), "bound to the setting name");
        for bad_token in ["", "v1.", "v1.AAAA", "v2.AAAA", "plain text", "v1.!!!!"] {
            assert!(c.decrypt("ai.key", bad_token).is_err(), "{bad_token:?}");
        }
    }

    #[test]
    fn key_file_is_created_private_reused_and_never_overwritten() {
        let p = tmp("key");
        let c1 = Crypto::from_key_file(&p).unwrap();
        let token = c1.encrypt("a", "v").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "owner-only");
        }
        // restart: same key, old ciphertext still readable
        assert_eq!(Crypto::from_key_file(&p).unwrap().decrypt("a", &token).unwrap(), "v");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text, "file untouched");
        // corrupt key file: loud failure, file preserved
        std::fs::write(&p, "not-a-key").unwrap();
        let err = Crypto::from_key_file(&p).err().unwrap();
        assert!(err.contains("refusing to overwrite"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "not-a-key");
        // key file in a missing directory is created
        let nested = tmp("dir").join("sub").join("platform.key");
        assert!(Crypto::from_key_file(&nested).is_ok() && nested.exists());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn settings_store_encrypted_values_and_never_plaintext() {
        let conn = create_test_db();
        let c = Crypto::for_tests();
        set(&conn, &c, "ai.api_key", "sk-very-secret", true, Some("admin1")).unwrap();
        set(&conn, &c, "brand.name", "منصة الامتحانات", false, None).unwrap();
        let raw: String = conn.query_row("SELECT value FROM settings WHERE key = 'ai.api_key'", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("sk-very-secret") && raw.starts_with("v1."), "stored encrypted: {raw}");
        assert_eq!(get(&conn, &c, "ai.api_key").unwrap().as_deref(), Some("sk-very-secret"));
        assert_eq!(get(&conn, &c, "brand.name").unwrap().as_deref(), Some("منصة الامتحانات"));
        assert_eq!(get(&conn, &c, "missing").unwrap(), None);
        assert!(is_set(&conn, "ai.api_key").unwrap() && !is_set(&conn, "missing").unwrap());
        set(&conn, &c, "ai.api_key", "sk-rotated", true, None).unwrap(); // overwrite
        assert_eq!(get(&conn, &c, "ai.api_key").unwrap().as_deref(), Some("sk-rotated"));
        let n: i64 = conn.query_row("SELECT count(*) FROM settings", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        delete(&conn, "ai.api_key").unwrap();
        assert_eq!(get(&conn, &c, "ai.api_key").unwrap(), None);
    }

    #[test]
    fn settings_reject_bad_keys_and_detect_swapped_rows() {
        let conn = create_test_db();
        let c = Crypto::for_tests();
        for k in ["", "Has Space", "UPPER", "a/b", &"k".repeat(65)] {
            assert_eq!(set(&conn, &c, k, "v", false, None).unwrap_err().0, axum::http::StatusCode::BAD_REQUEST, "{k:?}");
        }
        assert!(set(&conn, &c, "big", &"x".repeat(MAX_VALUE_CHARS + 1), false, None).is_err());
        // an attacker with DB write access copies one encrypted value onto another row
        set(&conn, &c, "ai.api_key", "key-A", true, None).unwrap();
        set(&conn, &c, "smtp.password", "pass-B", true, None).unwrap();
        conn.execute("UPDATE settings SET value = (SELECT value FROM settings WHERE key = 'ai.api_key') WHERE key = 'smtp.password'", []).unwrap();
        assert!(get(&conn, &c, "smtp.password").is_err(), "swapped ciphertext must not decrypt");
        assert_eq!(get(&conn, &c, "ai.api_key").unwrap().as_deref(), Some("key-A"));
    }
}
