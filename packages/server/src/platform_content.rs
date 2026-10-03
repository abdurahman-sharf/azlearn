//! Teacher content: articles/summaries (with one attachment), video courses (lessons) and
//! live sessions. Content is visible to students only while it is published AND its teacher is
//! an active teacher approved for that subject (revoking the assignment hides it automatically).

use crate::platform::{audit, bad, db_err, lock, new_id, opt_text, require_active, require_role, text, Res, User};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  original_name TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS posts (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('article','summary')),
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_posts_subject ON posts(subject_id, status);
CREATE INDEX IF NOT EXISTS idx_posts_teacher ON posts(teacher_id);
CREATE TABLE IF NOT EXISTS courses (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_courses_subject ON courses(subject_id, status);
CREATE TABLE IF NOT EXISTS lessons (
  id TEXT PRIMARY KEY,
  course_id TEXT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  section TEXT,
  title TEXT NOT NULL,
  description TEXT,
  video_url TEXT NOT NULL,
  embed_url TEXT,
  video_kind TEXT NOT NULL CHECK (video_kind IN ('youtube','vimeo','file','link')),
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lessons_course ON lessons(course_id, position);
CREATE TABLE IF NOT EXISTS live_sessions (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  starts_at INTEGER NOT NULL,
  duration_min INTEGER NOT NULL,
  join_url TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('scheduled','cancelled')),
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_live_subject ON live_sessions(subject_id, starts_at);
";

const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_BODY_CHARS: usize = 20_000;
const MAX_URL_CHARS: usize = 500;

// ───────── validation helpers ─────────

fn one_of<'a>(v: &'a str, allowed: &[&str], code: &str) -> Res<&'a str> {
    if allowed.contains(&v) {
        Ok(v)
    } else {
        Err(bad(code))
    }
}

/// Only https links, no whitespace/control characters.
fn https_url(raw: &str) -> Res<url::Url> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > MAX_URL_CHARS || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad("invalid_url"));
    }
    let u = url::Url::parse(t).map_err(|_| bad("invalid_url"))?;
    if u.scheme() != "https" || u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        return Err(bad("invalid_url"));
    }
    Ok(u)
}

fn valid_video_id(id: &str, len: Option<usize>) -> bool {
    !id.is_empty()
        && len.map_or(id.len() <= 20, |l| id.len() == l)
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Returns (kind, embed_url). The embed URL is computed here from a known provider, never
/// taken from user input, so the client can safely put it in an iframe.
pub fn parse_video(raw: &str) -> Res<(&'static str, Option<String>)> {
    let u = https_url(raw)?;
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.").to_lowercase();
    let segs: Vec<&str> = u.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    match host.as_str() {
        "youtube.com" => {
            let id = match segs.first().copied() {
                Some("watch") => u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned()),
                Some("embed") | Some("shorts") | Some("live") => segs.get(1).map(|s| s.to_string()),
                _ => None,
            };
            match id {
                Some(id) if valid_video_id(&id, Some(11)) => Ok(("youtube", Some(format!("https://www.youtube-nocookie.com/embed/{id}")))),
                _ => Ok(("link", None)),
            }
        }
        "youtu.be" => match segs.first() {
            Some(id) if valid_video_id(id, Some(11)) => Ok(("youtube", Some(format!("https://www.youtube-nocookie.com/embed/{id}")))),
            _ => Ok(("link", None)),
        },
        "vimeo.com" => match segs.first() {
            Some(id) if id.chars().all(|c| c.is_ascii_digit()) && valid_video_id(id, None) => {
                Ok(("vimeo", Some(format!("https://player.vimeo.com/video/{id}"))))
            }
            _ => Ok(("link", None)),
        },
        _ => {
            let path = u.path().to_lowercase();
            if [".mp4", ".webm", ".ogg"].iter().any(|e| path.ends_with(e)) {
                Ok(("file", Some(u.to_string())))
            } else {
                Ok(("link", None))
            }
        }
    }
}

/// An approved teaching assignment on an active subject is required to create/publish content.
fn can_publish(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM teacher_subjects ts JOIN subjects s ON s.id = ts.subject_id
         WHERE ts.teacher_id = ?1 AND ts.subject_id = ?2 AND ts.status = 'approved' AND s.is_active = 1)",
        params![teacher_id, subject_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

fn require_assignment(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<()> {
    if can_publish(conn, teacher_id, subject_id)? {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "not_assigned"))
    }
}

/// SQL condition: content `a` is published/scheduled AND its teacher is active and approved for the subject.
/// Expects aliases `u` (users) and `s` (subjects) in the query.
pub fn visible(a: &str, live_status: &str) -> String {
    format!(
        "{a}.status = '{live_status}' AND u.status = 'active' AND u.role = 'teacher' AND s.is_active = 1
         AND EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = {a}.teacher_id AND ts.subject_id = {a}.subject_id AND ts.status = 'approved')"
    )
}

#[derive(Clone, Copy)]
enum Scope<'a> {
    Subject(&'a str),
    Teacher(&'a str),
    Mine(&'a str),
    Feed(&'a str),
}

fn scope_clause(scope: Scope, a: &str, status: &str) -> String {
    match scope {
        Scope::Subject(_) => format!("{} AND {a}.subject_id = ?1", visible(a, status)),
        Scope::Teacher(_) => format!("{} AND {a}.teacher_id = ?1", visible(a, status)),
        Scope::Mine(_) => format!("{a}.teacher_id = ?1"),
        Scope::Feed(_) => format!(
            "{} AND ({a}.subject_id IN (SELECT subject_id FROM subject_enrollments WHERE student_id = ?1)
                  OR {a}.teacher_id IN (SELECT teacher_id FROM teacher_follows WHERE student_id = ?1))",
            visible(a, status)
        ),
    }
}

fn scope_id(scope: Scope) -> &str {
    match scope {
        Scope::Subject(i) | Scope::Teacher(i) | Scope::Mine(i) | Scope::Feed(i) => i,
    }
}

/// Tells followers/enrolled students about new content (only on first publication).
fn announce(conn: &Connection, teacher: &User, subject_id: &str, kind: &str, title: &str, link: &str) {
    crate::platform_engage::notify_audience(
        conn,
        &teacher.id,
        subject_id,
        kind,
        serde_json::json!({ "title": title, "teacher": teacher.full_name }),
        link,
    );
}

/// Moderation by someone other than the owner: tell the owner.
fn tell_owner_moderated(conn: &Connection, owner: &str, title: &str, link: &str) {
    crate::platform_engage::notify(conn, owner, "content_unpublished", serde_json::json!({ "title": title }), link);
}

fn can_manage(conn: &Connection, table: &str, id: &str, user: &User) -> Res<String> {
    // `table` is always a literal from this module.
    let owner: String = conn
        .query_row(&format!("SELECT teacher_id FROM {table} WHERE id = ?1"), params![id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if owner == user.id || user.role == "admin" {
        Ok(owner)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Non-owner admins may only unpublish/cancel (moderation), never edit.
fn admin_only_status(user: &User, owner: &str, patch_has_other_fields: bool) -> Res<()> {
    if user.id != owner && patch_has_other_fields {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

// ───────── posts ─────────

#[derive(Serialize, Debug)]
pub struct FileInfo {
    id: String,
    name: String,
    size: i64,
}

#[derive(Serialize, Debug)]
pub struct Post {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    kind: String,
    title: String,
    excerpt: String,
    body: Option<String>,
    status: String,
    file: Option<FileInfo>,
    created_at: i64,
    updated_at: i64,
}

const POST_SELECT: &str = "SELECT p.id, p.teacher_id, u.full_name, p.subject_id, s.name_ar, p.kind, p.title, p.body, p.status,
        p.file_id, f.original_name, f.size, p.created_at, p.updated_at
     FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id
     LEFT JOIN files f ON f.id = p.file_id";

fn map_post(with_body: bool) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Post> {
    move |r| {
        let body: String = r.get(7)?;
        let file = match (r.get::<_, Option<String>>(9)?, r.get::<_, Option<String>>(10)?, r.get::<_, Option<i64>>(11)?) {
            (Some(id), Some(name), Some(size)) => Some(FileInfo { id, name, size }),
            _ => None,
        };
        Ok(Post {
            id: r.get(0)?,
            teacher_id: r.get(1)?,
            teacher_name: r.get(2)?,
            subject_id: r.get(3)?,
            subject_name: r.get(4)?,
            kind: r.get(5)?,
            title: r.get(6)?,
            excerpt: body.chars().take(200).collect(),
            body: with_body.then_some(body),
            status: r.get(8)?,
            file,
            created_at: r.get(12)?,
            updated_at: r.get(13)?,
        })
    }
}

fn list_posts(conn: &Connection, scope: Scope) -> Res<Vec<Post>> {
    let sql = format!("{POST_SELECT} WHERE {} ORDER BY p.updated_at DESC LIMIT 100", scope_clause(scope, "p", "published"));
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![scope_id(scope)], map_post(false))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

fn get_post(conn: &Connection, id: &str) -> Res<Post> {
    conn.query_row(&format!("{POST_SELECT} WHERE p.id = ?1"), params![id], map_post(true))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn visible_to_public(conn: &Connection, table: &str, a: &str, id: &str, status: &str) -> Res<bool> {
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM {table} {a} JOIN users u ON u.id = {a}.teacher_id JOIN subjects s ON s.id = {a}.subject_id
             WHERE {a}.id = ?1 AND {})",
            visible(a, status)
        ),
        params![id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

#[derive(Deserialize)]
pub struct PostReq {
    subject_id: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    status: Option<String>,
}

fn create_post(conn: &Connection, user: &User, r: &PostReq) -> Res<Post> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let kind = one_of(r.kind.as_deref().unwrap_or("article"), &["article", "summary"], "invalid_kind")?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let body = text(r.body.as_deref().unwrap_or(""), MAX_BODY_CHARS, "invalid_body")?;
    let status = one_of(r.status.as_deref().unwrap_or("draft"), &["draft", "published"], "invalid_status")?;
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO posts(id, teacher_id, subject_id, kind, title, body, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, user.id, subject_id, kind, title, body, status, now],
    )
    .map_err(db_err)?;
    if status == "published" {
        announce(conn, user, subject_id, "new_post", &title, &format!("/platform/posts/{id}"));
    }
    get_post(conn, &id)
}

fn update_post(conn: &Connection, user: &User, id: &str, r: &PostReq) -> Res<Post> {
    let owner = can_manage(conn, "posts", id, user)?;
    let other = r.kind.is_some() || r.title.is_some() || r.body.is_some() || r.subject_id.is_some();
    admin_only_status(user, &owner, other)?;
    let cur = get_post(conn, id)?;
    let kind = one_of(r.kind.as_deref().unwrap_or(&cur.kind), &["article", "summary"], "invalid_kind")?.to_string();
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title.clone(),
    };
    let body = match &r.body {
        Some(b) => text(b, MAX_BODY_CHARS, "invalid_body")?,
        None => cur.body.clone().unwrap_or_default(),
    };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["draft", "published"], "invalid_status")?.to_string();
    if status == "published" && user.id == owner {
        require_assignment(conn, &owner, &cur.subject_id)?;
    }
    conn.execute(
        "UPDATE posts SET kind=?1, title=?2, body=?3, status=?4, updated_at=?5 WHERE id=?6",
        params![kind, title, body, status, now_ms(), id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "post_moderated", &status);
        if status == "draft" && cur.status == "published" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/posts/{id}"));
        }
    } else if status == "published" && cur.status != "published" {
        announce(conn, user, &cur.subject_id, "new_post", &title, &format!("/platform/posts/{id}"));
    }
    get_post(conn, id)
}

fn remove_file_row(state: &AppState, conn: &Connection, file_id: &str) {
    let _ = conn.execute("DELETE FROM files WHERE id = ?1", params![file_id]);
    let _ = std::fs::remove_file(state.platform.files_dir.join(file_id));
}

pub async fn create_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(r): Json<PostReq>,
) -> Res<(StatusCode, Json<Post>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_post(&*lock(&state)?, &user, &r).map(|p| (StatusCode::CREATED, Json(p)))
}

pub async fn get_post_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Post>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let post = get_post(&conn, &id)?;
    if post.teacher_id != user.id && user.role != "admin" && !visible_to_public(&conn, "posts", "p", &id, "published")? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    Ok(Json(post))
}

pub async fn update_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<PostReq>,
) -> Res<Json<Post>> {
    let user = require_active(&state, &headers)?;
    update_post(&*lock(&state)?, &user, &id, &r).map(Json)
}

pub async fn delete_post_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    can_manage(&conn, "posts", &id, &user)?;
    let file: Option<String> = conn.query_row("SELECT file_id FROM posts WHERE id = ?1", params![id], |r| r.get(0)).map_err(db_err)?;
    conn.execute("DELETE FROM posts WHERE id = ?1", params![id]).map_err(db_err)?;
    if let Some(f) = file {
        remove_file_row(&state, &conn, &f);
    }
    audit(&conn, &user.id, &id, "post_deleted", "");
    Ok(StatusCode::NO_CONTENT)
}

// ───────── attachments ─────────

/// (mime, magic-bytes check) for the extensions we accept. HTML/SVG/scripts are deliberately absent.
fn file_kind(ext: &str) -> Option<(&'static str, fn(&[u8]) -> bool)> {
    fn pdf(b: &[u8]) -> bool { b.starts_with(b"%PDF") }
    fn zip(b: &[u8]) -> bool { b.starts_with(b"PK\x03\x04") }
    fn png(b: &[u8]) -> bool { b.starts_with(b"\x89PNG\r\n\x1a\n") }
    fn jpg(b: &[u8]) -> bool { b.starts_with(&[0xFF, 0xD8, 0xFF]) }
    fn txt(b: &[u8]) -> bool { !b.contains(&0) }
    Some(match ext {
        "pdf" => ("application/pdf", pdf),
        "docx" => ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", zip),
        "pptx" => ("application/vnd.openxmlformats-officedocument.presentationml.presentation", zip),
        "xlsx" => ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", zip),
        "png" => ("image/png", png),
        "jpg" | "jpeg" => ("image/jpeg", jpg),
        "txt" => ("text/plain; charset=utf-8", txt),
        _ => return None,
    })
}

fn validate_upload(original: &str, bytes: &[u8]) -> Res<(&'static str, String)> {
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(bad("invalid_file_size"));
    }
    let base: String = original
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let (mime, magic) = file_kind(&ext).ok_or_else(|| bad("file_type_not_allowed"))?;
    if !magic(bytes) {
        return Err(bad("file_content_mismatch"));
    }
    Ok((mime, base))
}

pub async fn upload_file_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(post_id): Path<String>,
    mut mp: Multipart,
) -> Res<Json<FileInfo>> {
    let user = require_active(&state, &headers)?;
    {
        let conn = lock(&state)?;
        if can_manage(&conn, "posts", &post_id, &user)? != user.id {
            return Err(err(StatusCode::FORBIDDEN, "forbidden"));
        }
    }
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = mp.next_field().await.map_err(|_| bad("invalid_upload"))? {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("").to_string();
            let bytes = field.bytes().await.map_err(|_| bad("invalid_file_size"))?;
            upload = Some((name, bytes.to_vec()));
            break;
        }
    }
    let (name, bytes) = upload.ok_or_else(|| bad("invalid_upload"))?;
    let (mime, safe_name) = validate_upload(&name, &bytes)?;
    let file_id = new_id();
    std::fs::write(state.platform.files_dir.join(&file_id), &bytes).map_err(db_err)?;
    let conn = lock(&state)?;
    let old: Option<String> = conn.query_row("SELECT file_id FROM posts WHERE id = ?1", params![post_id], |r| r.get(0)).map_err(db_err)?;
    conn.execute(
        "INSERT INTO files(id, owner_id, original_name, mime, size, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![file_id, user.id, safe_name, mime, bytes.len() as i64, now_ms()],
    )
    .map_err(db_err)?;
    conn.execute("UPDATE posts SET file_id = ?1, updated_at = ?2 WHERE id = ?3", params![file_id, now_ms(), post_id]).map_err(db_err)?;
    if let Some(o) = old {
        remove_file_row(&state, &conn, &o);
    }
    Ok(Json(FileInfo { id: file_id, name: safe_name, size: bytes.len() as i64 }))
}

pub async fn remove_file_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(post_id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    if can_manage(&conn, "posts", &post_id, &user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let old: Option<String> = conn.query_row("SELECT file_id FROM posts WHERE id = ?1", params![post_id], |r| r.get(0)).map_err(db_err)?;
    conn.execute("UPDATE posts SET file_id = NULL WHERE id = ?1", params![post_id]).map_err(db_err)?;
    if let Some(o) = old {
        remove_file_row(&state, &conn, &o);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Download is allowed for the owner, admins, and anyone who can see the post.
pub async fn download_file_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(file_id): Path<String>) -> Res<Response> {
    let user = require_active(&state, &headers)?;
    let (name, mime): (String, String) = {
        let conn = lock(&state)?;
        let row = conn
            .query_row::<(String, String, String, String), _, _>(
                "SELECT f.original_name, f.mime, p.id, p.teacher_id FROM files f JOIN posts p ON p.file_id = f.id WHERE f.id = ?1",
                params![file_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
        if row.3 != user.id && user.role != "admin" && !visible_to_public(&conn, "posts", "p", &row.2, "published")? {
            return Err(err(StatusCode::NOT_FOUND, "not_found"));
        }
        (row.0, row.1)
    };
    let bytes = std::fs::read(state.platform.files_dir.join(&file_id)).map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    let encoded: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+', "%20");
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_DISPOSITION, format!("attachment; filename*=UTF-8''{encoded}")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CACHE_CONTROL, "private, max-age=0".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// Deletes file rows (and disk files) that no post references any more.
pub fn purge_orphan_files(state: &crate::platform::PlatformState) {
    let Ok(conn) = state.conn.lock() else { return };
    let cutoff = now_ms() - 3_600_000;
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM files WHERE created_at < ?1 AND id NOT IN (SELECT file_id FROM posts WHERE file_id IS NOT NULL)")
        .and_then(|mut s| s.query_map(params![cutoff], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for id in ids {
        let _ = conn.execute("DELETE FROM files WHERE id = ?1", params![id]);
        let _ = std::fs::remove_file(state.files_dir.join(&id));
    }
}

// ───────── courses & lessons ─────────

#[derive(Serialize, Debug)]
pub struct Course {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    title: String,
    description: Option<String>,
    status: String,
    lesson_count: i64,
    updated_at: i64,
}

#[derive(Serialize, Debug)]
pub struct Lesson {
    id: String,
    course_id: String,
    position: i64,
    section: Option<String>,
    title: String,
    description: Option<String>,
    video_url: String,
    embed_url: Option<String>,
    video_kind: String,
}

#[derive(Serialize, Debug)]
pub struct CourseDetail {
    #[serde(flatten)]
    course: Course,
    lessons: Vec<Lesson>,
}

const COURSE_SELECT: &str = "SELECT c.id, c.teacher_id, u.full_name, c.subject_id, s.name_ar, c.title, c.description, c.status,
        (SELECT count(*) FROM lessons l WHERE l.course_id = c.id), c.updated_at
     FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id";

fn map_course(r: &rusqlite::Row) -> rusqlite::Result<Course> {
    Ok(Course {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        subject_id: r.get(3)?,
        subject_name: r.get(4)?,
        title: r.get(5)?,
        description: r.get(6)?,
        status: r.get(7)?,
        lesson_count: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn list_courses(conn: &Connection, scope: Scope) -> Res<Vec<Course>> {
    let sql = format!("{COURSE_SELECT} WHERE {} ORDER BY c.updated_at DESC LIMIT 100", scope_clause(scope, "c", "published"));
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![scope_id(scope)], map_course)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

fn get_course(conn: &Connection, id: &str) -> Res<CourseDetail> {
    let course = conn
        .query_row(&format!("{COURSE_SELECT} WHERE c.id = ?1"), params![id], map_course)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let lessons = conn
        .prepare(
            "SELECT id, course_id, position, section, title, description, video_url, embed_url, video_kind
             FROM lessons WHERE course_id = ?1 ORDER BY position, created_at",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| {
            Ok(Lesson {
                id: r.get(0)?,
                course_id: r.get(1)?,
                position: r.get(2)?,
                section: r.get(3)?,
                title: r.get(4)?,
                description: r.get(5)?,
                video_url: r.get(6)?,
                embed_url: r.get(7)?,
                video_kind: r.get(8)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(CourseDetail { course, lessons })
}

#[derive(Deserialize)]
pub struct CourseReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

fn create_course(conn: &Connection, user: &User, r: &CourseReq) -> Res<CourseDetail> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let status = one_of(r.status.as_deref().unwrap_or("draft"), &["draft", "published"], "invalid_status")?;
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO courses(id, teacher_id, subject_id, title, description, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id, user.id, subject_id, title, description, status, now],
    )
    .map_err(db_err)?;
    if status == "published" {
        announce(conn, user, subject_id, "new_course", &title, &format!("/platform/courses/{id}"));
    }
    get_course(conn, &id)
}

fn update_course(conn: &Connection, user: &User, id: &str, r: &CourseReq) -> Res<CourseDetail> {
    let owner = can_manage(conn, "courses", id, user)?;
    admin_only_status(user, &owner, r.title.is_some() || r.description.is_some())?;
    let cur = get_course(conn, id)?.course;
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["draft", "published"], "invalid_status")?.to_string();
    if status == "published" && user.id == owner {
        require_assignment(conn, &owner, &cur.subject_id)?;
    }
    conn.execute(
        "UPDATE courses SET title=?1, description=?2, status=?3, updated_at=?4 WHERE id=?5",
        params![title, description, status, now_ms(), id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "course_moderated", &status);
        if status == "draft" && cur.status == "published" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/courses/{id}"));
        }
    } else if status == "published" && cur.status != "published" {
        announce(conn, user, &cur.subject_id, "new_course", &title, &format!("/platform/courses/{id}"));
    }
    get_course(conn, id)
}

#[derive(Deserialize)]
pub struct LessonReq {
    title: Option<String>,
    description: Option<String>,
    section: Option<String>,
    video_url: Option<String>,
}

fn add_lesson(conn: &Connection, user: &User, course_id: &str, r: &LessonReq) -> Res<Lesson> {
    if can_manage(conn, "courses", course_id, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let section = opt_text(&r.section, 120, "section_too_long")?;
    let (kind, embed) = parse_video(r.video_url.as_deref().unwrap_or(""))?;
    let url = r.video_url.as_deref().unwrap_or("").trim().to_string();
    let pos: i64 = conn
        .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM lessons WHERE course_id = ?1", params![course_id], |r| r.get(0))
        .map_err(db_err)?;
    let id = new_id();
    conn.execute(
        "INSERT INTO lessons(id, course_id, position, section, title, description, video_url, embed_url, video_kind, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![id, course_id, pos, section, title, description, url, embed, kind, now_ms()],
    )
    .map_err(db_err)?;
    conn.execute("UPDATE courses SET updated_at = ?1 WHERE id = ?2", params![now_ms(), course_id]).map_err(db_err)?;
    Ok(Lesson { id, course_id: course_id.into(), position: pos, section, title, description, video_url: url, embed_url: embed, video_kind: kind.into() })
}

fn lesson_course(conn: &Connection, lesson_id: &str) -> Res<String> {
    conn.query_row("SELECT course_id FROM lessons WHERE id = ?1", params![lesson_id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn update_lesson(conn: &Connection, user: &User, id: &str, r: &LessonReq) -> Res<()> {
    let course = lesson_course(conn, id)?;
    if can_manage(conn, "courses", &course, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let (title, description, section, url, embed, kind): (String, Option<String>, Option<String>, String, Option<String>, String) = conn
        .query_row(
            "SELECT title, description, section, video_url, embed_url, video_kind FROM lessons WHERE id = ?1",
            params![id],
            |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?)),
        )
        .map_err(db_err)?;
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { description };
    let section = if r.section.is_some() { opt_text(&r.section, 120, "section_too_long")? } else { section };
    let (url, embed, kind) = match &r.video_url {
        Some(u) => {
            let (k, e) = parse_video(u)?;
            (u.trim().to_string(), e, k.to_string())
        }
        None => (url, embed, kind),
    };
    conn.execute(
        "UPDATE lessons SET title=?1, description=?2, section=?3, video_url=?4, embed_url=?5, video_kind=?6 WHERE id=?7",
        params![title, description, section, url, embed, kind, id],
    )
    .map_err(db_err)?;
    Ok(())
}

fn delete_lesson(conn: &Connection, user: &User, id: &str) -> Res<()> {
    let course = lesson_course(conn, id)?;
    can_manage(conn, "courses", &course, user)?;
    conn.execute("DELETE FROM lessons WHERE id = ?1", params![id]).map_err(db_err)?;
    Ok(())
}

/// Swaps a lesson with its neighbour; `up` moves it earlier.
fn move_lesson(conn: &Connection, user: &User, id: &str, up: bool) -> Res<()> {
    let course = lesson_course(conn, id)?;
    if can_manage(conn, "courses", &course, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM lessons WHERE course_id = ?1 ORDER BY position, created_at")
        .map_err(db_err)?
        .query_map(params![course], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let i = ids.iter().position(|x| x == id).ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let j = if up { i.checked_sub(1) } else { (i + 1 < ids.len()).then_some(i + 1) };
    let mut order = ids;
    if let Some(j) = j {
        order.swap(i, j);
    }
    // Renumber densely so positions stay unique and stable.
    for (n, lid) in order.iter().enumerate() {
        conn.execute("UPDATE lessons SET position = ?1 WHERE id = ?2", params![n as i64 + 1, lid]).map_err(db_err)?;
    }
    Ok(())
}

pub async fn create_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<CourseReq>) -> Res<(StatusCode, Json<CourseDetail>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_course(&*lock(&state)?, &user, &r).map(|c| (StatusCode::CREATED, Json(c)))
}

pub async fn get_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<CourseDetail>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let c = get_course(&conn, &id)?;
    if c.course.teacher_id != user.id && user.role != "admin" && !visible_to_public(&conn, "courses", "c", &id, "published")? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    Ok(Json(c))
}

pub async fn update_course_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<CourseReq>,
) -> Res<Json<CourseDetail>> {
    let user = require_active(&state, &headers)?;
    update_course(&*lock(&state)?, &user, &id, &r).map(Json)
}

pub async fn delete_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    can_manage(&conn, "courses", &id, &user)?;
    conn.execute("DELETE FROM courses WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(&conn, &user.id, &id, "course_deleted", "");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(course_id): Path<String>,
    Json(r): Json<LessonReq>,
) -> Res<(StatusCode, Json<Lesson>)> {
    let user = require_role(&state, &headers, "teacher")?;
    add_lesson(&*lock(&state)?, &user, &course_id, &r).map(|l| (StatusCode::CREATED, Json(l)))
}

pub async fn update_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<LessonReq>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "teacher")?;
    update_lesson(&*lock(&state)?, &user, &id, &r)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_lesson_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    delete_lesson(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct MoveReq {
    direction: String,
}

pub async fn move_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<MoveReq>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "teacher")?;
    let up = one_of(&r.direction, &["up", "down"], "invalid_direction")? == "up";
    move_lesson(&*lock(&state)?, &user, &id, up)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── live sessions ─────────

#[derive(Serialize, Debug)]
pub struct Live {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    title: String,
    description: Option<String>,
    starts_at: i64,
    duration_min: i64,
    join_url: String,
    status: String,
}

const LIVE_SELECT: &str = "SELECT l.id, l.teacher_id, u.full_name, l.subject_id, s.name_ar, l.title, l.description,
        l.starts_at, l.duration_min, l.join_url, l.status
     FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id";

fn map_live(r: &rusqlite::Row) -> rusqlite::Result<Live> {
    Ok(Live {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        subject_id: r.get(3)?,
        subject_name: r.get(4)?,
        title: r.get(5)?,
        description: r.get(6)?,
        starts_at: r.get(7)?,
        duration_min: r.get(8)?,
        join_url: r.get(9)?,
        status: r.get(10)?,
    })
}

/// Public scopes only list sessions that have not ended yet; `Mine` lists everything.
fn list_live(conn: &Connection, scope: Scope, now: i64) -> Res<Vec<Live>> {
    let not_ended = if matches!(scope, Scope::Mine(_)) { "" } else { " AND l.starts_at + l.duration_min * 60000 > ?2" };
    let sql = format!(
        "{LIVE_SELECT} WHERE {}{not_ended} ORDER BY l.starts_at {} LIMIT 100",
        scope_clause(scope, "l", "scheduled"),
        if matches!(scope, Scope::Mine(_)) { "DESC" } else { "ASC" }
    );
    let mut stmt = conn.prepare(&sql).map_err(db_err)?;
    let rows = if matches!(scope, Scope::Mine(_)) {
        stmt.query_map(params![scope_id(scope)], map_live)
    } else {
        stmt.query_map(params![scope_id(scope), now], map_live)
    }
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)?;
    Ok(rows)
}

#[derive(Deserialize)]
pub struct LiveReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    starts_at: Option<i64>,
    duration_min: Option<i64>,
    join_url: Option<String>,
    status: Option<String>,
}

fn check_live_time(starts_at: i64, duration: i64) -> Res<()> {
    let now = now_ms();
    if !(5..=480).contains(&duration) || starts_at < now - 3_600_000 || starts_at > now + 366 * 86_400_000 {
        return Err(bad("invalid_time"));
    }
    Ok(())
}

fn get_live(conn: &Connection, id: &str) -> Res<Live> {
    conn.query_row(&format!("{LIVE_SELECT} WHERE l.id = ?1"), params![id], map_live)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn create_live(conn: &Connection, user: &User, r: &LiveReq) -> Res<Live> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let starts_at = r.starts_at.ok_or_else(|| bad("invalid_time"))?;
    let duration = r.duration_min.unwrap_or(60);
    check_live_time(starts_at, duration)?;
    let url = https_url(r.join_url.as_deref().unwrap_or(""))?.to_string();
    let id = new_id();
    conn.execute(
        "INSERT INTO live_sessions(id, teacher_id, subject_id, title, description, starts_at, duration_min, join_url, status, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'scheduled',?9)",
        params![id, user.id, subject_id, title, description, starts_at, duration, url, now_ms()],
    )
    .map_err(db_err)?;
    announce(conn, user, subject_id, "live_scheduled", &title, &format!("/platform/subjects/{subject_id}"));
    get_live(conn, &id)
}

fn update_live(conn: &Connection, user: &User, id: &str, r: &LiveReq) -> Res<Live> {
    let owner = can_manage(conn, "live_sessions", id, user)?;
    admin_only_status(user, &owner, r.title.is_some() || r.description.is_some() || r.starts_at.is_some() || r.duration_min.is_some() || r.join_url.is_some())?;
    let cur = get_live(conn, id)?;
    let (cur_status, cur_subject, cur_teacher) = (cur.status.clone(), cur.subject_id.clone(), cur.teacher_name.clone());
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description };
    let starts_at = r.starts_at.unwrap_or(cur.starts_at);
    let duration = r.duration_min.unwrap_or(cur.duration_min);
    if r.starts_at.is_some() || r.duration_min.is_some() {
        check_live_time(starts_at, duration)?;
    }
    let url = match &r.join_url {
        Some(u) => https_url(u)?.to_string(),
        None => cur.join_url,
    };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["scheduled", "cancelled"], "invalid_status")?.to_string();
    conn.execute(
        "UPDATE live_sessions SET title=?1, description=?2, starts_at=?3, duration_min=?4, join_url=?5, status=?6 WHERE id=?7",
        params![title, description, starts_at, duration, url, status, id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "live_moderated", &status);
        if status == "cancelled" && cur_status == "scheduled" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/subjects/{}", cur_subject));
        }
    }
    if status == "cancelled" && cur_status == "scheduled" {
        let teacher = User { id: owner.clone(), email: String::new(), full_name: cur_teacher, role: "teacher".into(), institution_type: None, status: "active".into(), status_reason: None };
        announce(conn, &teacher, &cur_subject, "live_cancelled", &title, &format!("/platform/subjects/{cur_subject}"));
    }
    get_live(conn, id)
}

pub async fn create_live_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<LiveReq>) -> Res<(StatusCode, Json<Live>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_live(&*lock(&state)?, &user, &r).map(|l| (StatusCode::CREATED, Json(l)))
}

pub async fn update_live_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<LiveReq>,
) -> Res<Json<Live>> {
    let user = require_active(&state, &headers)?;
    update_live(&*lock(&state)?, &user, &id, &r).map(Json)
}

pub async fn delete_live_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    can_manage(&conn, "live_sessions", &id, &user)?;
    conn.execute("DELETE FROM live_sessions WHERE id = ?1", params![id]).map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── bundles ─────────

#[derive(Serialize)]
pub struct Bundle {
    posts: Vec<Post>,
    courses: Vec<Course>,
    live: Vec<Live>,
}

fn bundle(conn: &Connection, scope: Scope) -> Res<Bundle> {
    Ok(Bundle { posts: list_posts(conn, scope)?, courses: list_courses(conn, scope)?, live: list_live(conn, scope, now_ms())? })
}

pub async fn subject_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Bundle>> {
    require_active(&state, &headers)?;
    bundle(&*lock(&state)?, Scope::Subject(&id)).map(Json)
}

pub async fn teacher_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Bundle>> {
    require_active(&state, &headers)?;
    bundle(&*lock(&state)?, Scope::Teacher(&id)).map(Json)
}

pub async fn my_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Bundle>> {
    let user = require_role(&state, &headers, "teacher")?;
    bundle(&*lock(&state)?, Scope::Mine(&user.id)).map(Json)
}

/// Student home feed: content/live from enrolled subjects and followed teachers.
pub async fn feed_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Bundle>> {
    let user = require_role(&state, &headers, "student")?;
    let mut b = bundle(&*lock(&state)?, Scope::Feed(&user.id))?;
    b.posts.truncate(10);
    b.courses.truncate(10);
    Ok(Json(b))
}

/// Router layer for the upload route (default body limit is 2 MB).
pub fn upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_FILE_BYTES + 512 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    struct World {
        conn: Connection,
        teacher: User,
        student: User,
        admin: User,
        subject: String,
    }

    fn world() -> World {
        let conn = create_test_db();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0)", []).unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO teacher_subjects VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s1',0)", params![student.id]).unwrap();
        World { conn, teacher, student, admin, subject: "s1".into() }
    }

    fn post_req(w: &World, status: &str) -> PostReq {
        PostReq { subject_id: Some(w.subject.clone()), kind: Some("summary".into()), title: Some("ملخص".into()), body: Some("نص".into()), status: Some(status.into()) }
    }

    #[test]
    fn video_urls_are_classified_and_sanitized() {
        let y = |u: &str| parse_video(u).unwrap();
        assert_eq!(y("https://www.youtube.com/watch?v=dQw4w9WgXcQ").1.as_deref(), Some("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"));
        assert_eq!(y("https://youtu.be/dQw4w9WgXcQ?t=5").0, "youtube");
        assert_eq!(y("https://youtube.com/embed/dQw4w9WgXcQ").0, "youtube");
        assert_eq!(y("https://vimeo.com/123456").1.as_deref(), Some("https://player.vimeo.com/video/123456"));
        assert_eq!(y("https://cdn.example.com/v/lesson.MP4").0, "file");
        assert_eq!(y("https://example.com/page"), ("link", None));
        assert_eq!(y("https://www.youtube.com/watch?v=bad\"onload"), ("link", None)); // bad id never embedded
        for bad_url in ["http://youtu.be/dQw4w9WgXcQ", "javascript:alert(1)", "https://u:p@example.com/", "https://exa mple.com", "", "ftp://x.com/a.mp4"] {
            assert!(parse_video(bad_url).is_err(), "{bad_url}");
        }
    }

    #[test]
    fn creating_content_requires_approved_assignment() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(create_post(&w.conn, &other, &post_req(&w, "draft")).unwrap_err().0, StatusCode::FORBIDDEN);
        w.conn.execute("INSERT INTO teacher_subjects VALUES (?1,'s1','pending',0,NULL)", params![other.id]).unwrap();
        assert_eq!(create_post(&w.conn, &other, &post_req(&w, "draft")).unwrap_err().0, StatusCode::FORBIDDEN, "pending is not enough");
        assert!(create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).is_ok());
    }

    #[test]
    fn visibility_follows_status_and_assignment() {
        let w = world();
        let draft = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        let live_post = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        let public = || list_posts(&w.conn, Scope::Subject("s1")).unwrap().len();
        assert_eq!(public(), 1, "drafts hidden");
        assert!(!visible_to_public(&w.conn, "posts", "p", &draft.id, "published").unwrap());
        assert_eq!(list_posts(&w.conn, Scope::Mine(&w.teacher.id)).unwrap().len(), 2);
        assert_eq!(list_posts(&w.conn, Scope::Feed(&w.student.id)).unwrap().len(), 1, "enrolled student sees it in feed");
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert_eq!(public(), 0, "revoking the assignment hides content");
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        w.conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![w.teacher.id]).unwrap();
        assert_eq!(public(), 0, "suspended teacher's content hidden");
        assert!(!visible_to_public(&w.conn, "posts", "p", &live_post.id, "published").unwrap());
    }

    #[test]
    fn only_owner_edits_and_admin_only_moderates_status() {
        let w = world();
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        let edit = |title: &str| PostReq { subject_id: None, kind: None, title: Some(title.into()), body: None, status: None };
        assert_eq!(update_post(&w.conn, &w.student, &p.id, &edit("x")).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(update_post(&w.conn, &w.admin, &p.id, &edit("x")).unwrap_err().0, StatusCode::FORBIDDEN, "admin cannot rewrite");
        let unpublish = PostReq { subject_id: None, kind: None, title: None, body: None, status: Some("draft".into()) };
        assert_eq!(update_post(&w.conn, &w.admin, &p.id, &unpublish).unwrap().status, "draft");
        assert_eq!(list_posts(&w.conn, Scope::Subject("s1")).unwrap().len(), 0);
        assert_eq!(update_post(&w.conn, &w.teacher, &p.id, &edit("جديد")).unwrap().title, "جديد");
        let audit_n: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'post_moderated'", [], |r| r.get(0)).unwrap();
        assert_eq!(audit_n, 1);
    }

    #[test]
    fn upload_validation() {
        assert!(validate_upload("a.pdf", b"%PDF-1.4 x").is_ok());
        assert_eq!(validate_upload("a.pdf", b"<html>").unwrap_err().0, StatusCode::BAD_REQUEST, "magic mismatch");
        assert_eq!(validate_upload("evil.html", b"<html>").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("evil.svg", b"<svg>").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("a.png", b"").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("a.pdf", &vec![b'%'; MAX_FILE_BYTES + 1]).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("../../etc/pass wd.docx", b"PK\x03\x04..").unwrap().1, "pass wd.docx", "path stripped");
        assert!(validate_upload("noext", b"x").is_err());
    }

    #[test]
    fn course_lessons_order_and_move() {
        let w = world();
        let c = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: Some("published".into()) }).unwrap();
        let add = |t: &str| add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some(t.into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        let (a, b, cc) = (add("أ"), add("ب"), add("ج"));
        let titles = || get_course(&w.conn, &c.course.id).unwrap().lessons.iter().map(|l| l.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(), ["أ", "ب", "ج"]);
        move_lesson(&w.conn, &w.teacher, &cc.id, true).unwrap();
        assert_eq!(titles(), ["أ", "ج", "ب"]);
        move_lesson(&w.conn, &w.teacher, &a.id, true).unwrap(); // already first: no-op
        move_lesson(&w.conn, &w.teacher, &b.id, false).unwrap(); // already last: no-op
        assert_eq!(titles(), ["أ", "ج", "ب"]);
        assert_eq!(add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some("x".into()), description: None, section: None, video_url: Some("http://x.com/a.mp4".into()) }).unwrap_err().0, StatusCode::BAD_REQUEST);
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(move_lesson(&w.conn, &other, &a.id, false).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(list_courses(&w.conn, Scope::Subject("s1")).unwrap()[0].lesson_count, 3);
        delete_lesson(&w.conn, &w.teacher, &b.id).unwrap();
        assert_eq!(titles(), ["أ", "ج"]);
    }

    #[test]
    fn live_sessions_validate_and_expire_from_listing() {
        let w = world();
        let now = now_ms();
        let req = |starts: i64, dur: i64, url: &str| LiveReq { subject_id: Some("s1".into()), title: Some("درس".into()), description: None, starts_at: Some(starts), duration_min: Some(dur), join_url: Some(url.into()), status: None };
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now + 1000, 60, "http://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now + 1000, 2, "https://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now - 10 * 86_400_000, 60, "https://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        let l = create_live(&w.conn, &w.teacher, &req(now + 3_600_000, 60, "https://meet.example.com/x")).unwrap();
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now).unwrap().len(), 1);
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now + 3 * 3_600_000).unwrap().len(), 0, "ended sessions drop off");
        let cancel = LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: None, status: Some("cancelled".into()) };
        update_live(&w.conn, &w.admin, &l.id, &cancel).unwrap();
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now).unwrap().len(), 0);
        assert_eq!(list_live(&w.conn, Scope::Mine(&w.teacher.id), now).unwrap().len(), 1);
    }

    #[test]
    fn deleting_post_cascades_and_orphan_files_purged() {
        let w = world();
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        w.conn.execute("INSERT INTO files VALUES ('f1', ?1, 'a.pdf', 'application/pdf', 4, 0)", params![w.teacher.id]).unwrap();
        w.conn.execute("UPDATE posts SET file_id = 'f1' WHERE id = ?1", params![p.id]).unwrap();
        assert_eq!(get_post(&w.conn, &p.id).unwrap().file.unwrap().name, "a.pdf");
        w.conn.execute("DELETE FROM subjects WHERE id = 's1'", []).unwrap();
        let n: i64 = w.conn.query_row("SELECT count(*) FROM posts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "posts cascade with the subject");
        let state = crate::platform::PlatformState { conn: std::sync::Mutex::new(w.conn), files_dir: std::env::temp_dir() };
        purge_orphan_files(&state);
        let files: i64 = state.conn.lock().unwrap().query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(files, 0);
    }
}
