//! Backups, restore and system status (phase 1-9, PRD I2–I4).
//!
//! A backup is one ZIP file holding a *consistent* snapshot of the platform database (`VACUUM INTO`, taken
//! while the server keeps running), the master key (`platform.key`, so encrypted settings stay readable) and
//! every uploaded attachment, plus a `manifest.json` with a SHA-256 for each entry and row counts of the main
//! tables. Restoring is an offline operation (`exameow-server restore <file>`): the backup is verified
//! end to end in a staging directory first, the current data is moved aside (never deleted), and only then is
//! the restored data moved into place — a failure at any step leaves the previous data as it was.
//!
//! The backup directory is the source of truth for which backups exist (`reconcile` runs at start), so a
//! restore of an older snapshot never loses track of newer backup files.

use crate::platform::{audit, bad, db_err, lock, new_id, rate_limit, require_admin, PlatformState, Res};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    body::Body,
    extract::{Path as AxPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS backups (
  id TEXT PRIMARY KEY,
  file TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('manual','nightly')),
  size INTEGER NOT NULL DEFAULT 0,
  files_count INTEGER NOT NULL DEFAULT 0,
  ok INTEGER NOT NULL DEFAULT 1,
  error TEXT,
  created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_backups_kind ON backups(kind, created_at);
";

const FORMAT: i64 = 1;
const DB_ENTRY: &str = "platform.db";
const KEY_ENTRY: &str = "platform.key";
const FILES_PREFIX: &str = "files/";
const MANIFEST: &str = "manifest.json";
/// Tables whose row counts are recorded in the manifest and compared after a restore.
const COUNTED: [&str; 12] = [
    "users", "institutions", "subjects", "assessments", "attempts", "question_bank_items",
    "posts", "courses", "lessons", "notifications", "audit_log", "settings",
];
const MIB: u64 = 1024 * 1024;
const LOW_DISK_BYTES: u64 = 500 * MIB;
const FAILED_KEEP_MS: i64 = 30 * 86_400_000;
const STALE_PART_MS: i64 = 3_600_000;
/// A nightly backup that failed is retried no sooner than this.
const RETRY_AFTER_FAILURE_MS: i64 = 3 * 3_600_000;
const OVERDUE_NIGHTLY_MS: i64 = 36 * 3_600_000;
const OVERDUE_MANUAL_ONLY_MS: i64 = 7 * 86_400_000;
const VERSION: &str = env!("CARGO_PKG_VERSION");

// ───────── layout & configuration ─────────

/// Where everything lives on disk. Built from the environment (same defaults as `main`).
#[derive(Clone, Debug)]
pub struct Layout {
    pub db_path: PathBuf,
    pub files_dir: PathBuf,
    pub key_file: PathBuf,
    pub backup_dir: PathBuf,
}

impl Layout {
    pub fn from_env() -> Layout {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let db = var("PLATFORM_DB_PATH").unwrap_or_else(|| "./exameow-platform.db".into());
        let files = var("PLATFORM_FILES_DIR").unwrap_or_else(|| "./platform-files".into());
        let parent = data_dir_of(Path::new(&db));
        Layout {
            db_path: db.into(),
            files_dir: files.into(),
            key_file: var("PLATFORM_KEY_FILE").map(PathBuf::from).unwrap_or_else(|| parent.join("platform.key")),
            backup_dir: var("PLATFORM_BACKUP_DIR").map(PathBuf::from).unwrap_or_else(|| parent.join("backups")),
        }
    }

    fn data_dir(&self) -> PathBuf {
        data_dir_of(&self.db_path)
    }
}

fn data_dir_of(db: &Path) -> PathBuf {
    db.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Clone, Debug)]
pub struct BackupConfig {
    pub keep_nightly: usize,
    pub keep_manual: usize,
    /// UTC hour of the nightly backup; `None` turns the nightly job off.
    pub nightly_hour: Option<u32>,
}

impl Default for BackupConfig {
    fn default() -> Self {
        BackupConfig { keep_nightly: 7, keep_manual: 5, nightly_hour: Some(3) }
    }
}

impl BackupConfig {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        Self::parse(var("PLATFORM_BACKUP_HOUR").as_deref(), var("PLATFORM_BACKUP_KEEP").as_deref(), var("PLATFORM_BACKUP_KEEP_MANUAL").as_deref())
    }

    /// `hour`: `0`–`23` (UTC) or `off`; `keep` / `keep_manual`: how many good backups of each kind to retain.
    /// Anything unparseable falls back to the default rather than switching a safety net off by accident.
    pub fn parse(hour: Option<&str>, keep: Option<&str>, keep_manual: Option<&str>) -> Self {
        let mut c = BackupConfig::default();
        let clean = |v: Option<&str>| v.map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty());
        if let Some(h) = clean(hour) {
            c.nightly_hour = match h.as_str() {
                "off" | "none" | "false" | "disabled" => None,
                other => other.parse::<u32>().ok().filter(|h| *h < 24).or(c.nightly_hour),
            };
        }
        if let Some(n) = clean(keep).and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=365).contains(n)) {
            c.keep_nightly = n;
        }
        if let Some(n) = clean(keep_manual).and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=365).contains(n)) {
            c.keep_manual = n;
        }
        c
    }
}

// ───────── small helpers ─────────

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// (year, month, day, hour, minute, second) in UTC from epoch milliseconds (no date library).
fn civil_utc(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

fn stamp(ms: i64) -> String {
    let (y, m, d, h, mi, s) = civil_utc(ms);
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Backup file names are generated by us; anything else (path separators, odd characters) is refused before
/// a name from the database or the URL ever touches the file system.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name.ends_with(".zip")
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.contains("..")
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn make_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// (free, total) bytes of the volume holding `path`; `None` when it cannot be determined.
#[cfg(unix)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        if !probe.pop() {
            return None;
        }
    }
    let c = std::ffi::CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let unit = st.f_frsize as u64;
    Some((st.f_bavail as u64 * unit, st.f_blocks as u64 * unit))
}

#[cfg(not(unix))]
pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Total size and count of regular files under `dir` (symlinks are not followed).
fn dir_stats(dir: &Path) -> (u64, u64) {
    let (mut bytes, mut count) = (0, 0);
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_file() {
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                count += 1;
            } else if t.is_dir() && depth < 8 {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    (bytes, count)
}

/// Regular files under `dir` as (relative path with `/`, absolute path), sorted; symlinks are skipped.
fn list_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new(), 0usize)];
    while let Some((d, rel, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
            let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if t.is_file() {
                out.push((rel_name, e.path()));
            } else if t.is_dir() && depth < 8 {
                stack.push((e.path(), rel_name, depth + 1));
            }
        }
    }
    out.sort();
    out
}

/// Bytes the database occupies (pages × page size), independent of whether it lives in a file.
fn db_bytes(conn: &Connection) -> u64 {
    let n = |p: &str| conn.query_row(&format!("PRAGMA {p}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0).max(0) as u64;
    n("page_count") * n("page_size")
}

fn row_counts(conn: &Connection) -> BTreeMap<String, i64> {
    COUNTED
        .iter()
        .map(|t| (t.to_string(), conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0)).unwrap_or(-1)))
        .collect()
}

/// Disk space the backup needs: the snapshot copy, the zip (at worst as big as its inputs) and a margin.
fn required_bytes(db: u64, files: u64) -> u64 {
    db * 2 + files + 8 * MIB
}

// ───────── creating a backup ─────────

#[derive(Serialize, Clone, Debug)]
pub struct BackupInfo {
    pub id: String,
    pub kind: String,
    pub file: String,
    pub created_at: i64,
    pub size: i64,
    pub files_count: i64,
    pub ok: bool,
    pub error: Option<String>,
    /// False when the file is gone from the backup directory (deleted by hand, or not copied back).
    pub present: bool,
    pub created_by: Option<String>,
}

struct Fail {
    code: &'static str,
    message: String,
}

impl Fail {
    fn io(context: &str, e: impl std::fmt::Display) -> Fail {
        Fail { code: "backup_failed", message: format!("{context}: {e}") }
    }
}

struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct EntryMeta {
    name: String,
    size: u64,
    sha256: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Manifest {
    format: i64,
    app: String,
    kind: String,
    created_at: i64,
    server_version: String,
    entries: Vec<EntryMeta>,
    counts: BTreeMap<String, i64>,
}

/// Streams `path` into the archive as `name`, returning its size and SHA-256.
fn add_entry(zw: &mut ZipWriter<File>, name: &str, path: &Path, method: CompressionMethod) -> Result<EntryMeta, Fail> {
    let mut f = File::open(path).map_err(|e| Fail::io(name, e))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let opts = SimpleFileOptions::default().compression_method(method).large_file(len >= 3 << 30).unix_permissions(0o600);
    zw.start_file(name, opts).map_err(|e| Fail::io(name, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| Fail::io(name, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        zw.write_all(&buf[..n]).map_err(|e| Fail::io(name, e))?;
        total += n as u64;
    }
    Ok(EntryMeta { name: name.to_string(), size: total, sha256: hex(&hasher.finalize()) })
}

struct Built {
    file: String,
    size: u64,
    files_count: i64,
}

fn build(state: &PlatformState, id: &str, kind: &str, now: i64) -> Result<Built, Fail> {
    let l = &state.layout;
    ensure_private_dir(&l.backup_dir).map_err(|e| Fail::io("backup directory", e))?;
    let (db, files) = {
        let conn = state.conn.lock().map_err(|_| Fail::io("database", "lock poisoned"))?;
        (db_bytes(&conn), dir_stats(&l.files_dir).0)
    };
    if let Some((free, _)) = disk_space(&l.backup_dir) {
        let need = required_bytes(db, files);
        if free < need {
            return Err(Fail { code: "no_space", message: format!("needs about {} MiB free, {} MiB available", need / MIB, free / MIB) });
        }
    }
    let mut file = format!("exameow-backup-{}-{kind}.zip", stamp(now));
    if l.backup_dir.join(&file).exists() {
        file = format!("exameow-backup-{}-{kind}-{}.zip", stamp(now), &id[..6]);
    }
    let final_path = l.backup_dir.join(&file);
    let part = l.backup_dir.join(format!("{file}.part"));
    let snapshot = l.backup_dir.join(format!("tmp-{id}.db"));

    let result = (|| -> Result<Built, Fail> {
        // 1. a consistent copy of the database; the connection is held only while SQLite writes it
        {
            let conn = state.conn.lock().map_err(|_| Fail::io("database", "lock poisoned"))?;
            conn.execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()]).map_err(|e| Fail::io("database snapshot", e))?;
        }
        make_private(&snapshot);
        let counts = {
            let snap = Connection::open_with_flags(&snapshot, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| Fail::io("database snapshot", e))?;
            row_counts(&snap)
        };
        // 2. the archive: database, master key, attachments, manifest (written last, once checksums are known)
        let out = File::create(&part).map_err(|e| Fail::io("archive", e))?;
        make_private(&part);
        let mut zw = ZipWriter::new(out);
        let mut entries = vec![add_entry(&mut zw, DB_ENTRY, &snapshot, CompressionMethod::Deflated)?];
        if l.key_file.is_file() {
            entries.push(add_entry(&mut zw, KEY_ENTRY, &l.key_file, CompressionMethod::Stored)?);
        }
        let mut files_count = 0;
        for (rel, path) in list_files(&l.files_dir) {
            match add_entry(&mut zw, &format!("{FILES_PREFIX}{rel}"), &path, CompressionMethod::Stored) {
                Ok(e) => {
                    entries.push(e);
                    files_count += 1;
                }
                // an attachment deleted after the snapshot was taken is no longer referenced by it
                Err(f) if !path.exists() => drop(f),
                Err(f) => return Err(f),
            }
        }
        let manifest = Manifest { format: FORMAT, app: "exameow-platform".into(), kind: kind.into(), created_at: now, server_version: VERSION.into(), entries, counts };
        zw.start_file(MANIFEST, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).unix_permissions(0o600)).map_err(|e| Fail::io(MANIFEST, e))?;
        zw.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| Fail::io(MANIFEST, e))?).map_err(|e| Fail::io(MANIFEST, e))?;
        let out = zw.finish().map_err(|e| Fail::io("archive", e))?;
        out.sync_all().map_err(|e| Fail::io("archive", e))?;
        drop(out);
        fs::rename(&part, &final_path).map_err(|e| Fail::io("archive", e))?;
        let size = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
        Ok(Built { file: file.clone(), size, files_count })
    })();
    let _ = fs::remove_file(&snapshot);
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Creates a backup now. `by` is the admin who asked (`None` for the nightly job). Refuses to run two at once.
pub fn create_backup(state: &PlatformState, kind: &str, by: Option<&str>, now: i64) -> Res<BackupInfo> {
    if !matches!(kind, "manual" | "nightly") {
        return Err(bad("invalid_kind"));
    }
    if state.backup_busy.swap(true, Ordering::SeqCst) {
        return Err(err(StatusCode::CONFLICT, "backup_running"));
    }
    let _busy = Busy(&state.backup_busy);
    let id = new_id();
    let outcome = build(state, &id, kind, now);
    let conn = state.conn.lock().map_err(db_err)?;
    match outcome {
        Ok(b) => {
            conn.execute(
                "INSERT INTO backups(id, file, kind, size, files_count, ok, created_by, created_at) VALUES (?1,?2,?3,?4,?5,1,?6,?7)",
                params![id, b.file, kind, b.size as i64, b.files_count, by, now],
            )
            .map_err(db_err)?;
            audit(&conn, by.unwrap_or(""), &id, "backup_created", kind);
            prune(&conn, &state.layout.backup_dir, kind, if kind == "nightly" { state.backup.keep_nightly } else { state.backup.keep_manual }, now);
            get_info(&conn, &state.layout.backup_dir, &id)
        }
        Err(f) => {
            let msg: String = f.message.chars().take(300).collect();
            let _ = conn.execute(
                "INSERT INTO backups(id, file, kind, size, ok, error, created_by, created_at) VALUES (?1,'',?2,0,0,?3,?4,?5)",
                params![id, kind, format!("{}: {msg}", f.code), by, now],
            );
            audit(&conn, by.unwrap_or(""), &id, "backup_failed", f.code);
            prune(&conn, &state.layout.backup_dir, kind, usize::MAX, now);
            Err(err(if f.code == "no_space" { StatusCode::INSUFFICIENT_STORAGE } else { StatusCode::INTERNAL_SERVER_ERROR }, f.code))
        }
    }
}

/// Keeps the newest `keep` good backups of `kind` (files and rows) and forgets failure records after 30 days.
fn prune(conn: &Connection, dir: &Path, kind: &str, keep: usize, now: i64) {
    let _ = conn.execute("DELETE FROM backups WHERE ok = 0 AND created_at < ?1", params![now - FAILED_KEEP_MS]);
    if keep == usize::MAX {
        return;
    }
    let old: Vec<(String, String)> = conn
        .prepare("SELECT id, file FROM backups WHERE kind = ?1 AND ok = 1 ORDER BY created_at DESC, rowid DESC LIMIT -1 OFFSET ?2")
        .and_then(|mut s| s.query_map(params![kind, keep as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .unwrap_or_default();
    for (id, file) in old {
        if safe_file_name(&file) {
            let _ = fs::remove_file(dir.join(&file));
        }
        let _ = conn.execute("DELETE FROM backups WHERE id = ?1", params![id]);
    }
}

const INFO_COLS: &str = "b.id, b.kind, b.file, b.created_at, b.size, b.files_count, b.ok, b.error, u.full_name";

fn map_info(dir: &Path, r: &rusqlite::Row) -> rusqlite::Result<BackupInfo> {
    let file: String = r.get(2)?;
    let ok = r.get::<_, i64>(6)? != 0;
    Ok(BackupInfo {
        id: r.get(0)?,
        kind: r.get(1)?,
        present: ok && safe_file_name(&file) && dir.join(&file).is_file(),
        file,
        created_at: r.get(3)?,
        size: r.get(4)?,
        files_count: r.get(5)?,
        ok,
        error: r.get(7)?,
        created_by: r.get(8)?,
    })
}

fn get_info(conn: &Connection, dir: &Path, id: &str) -> Res<BackupInfo> {
    conn.query_row(&format!("SELECT {INFO_COLS} FROM backups b LEFT JOIN users u ON u.id = b.created_by WHERE b.id = ?1"), params![id], |r| map_info(dir, r))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

pub fn list_backups(conn: &Connection, dir: &Path, limit: i64) -> Res<Vec<BackupInfo>> {
    conn.prepare(&format!("SELECT {INFO_COLS} FROM backups b LEFT JOIN users u ON u.id = b.created_by ORDER BY b.created_at DESC, b.rowid DESC LIMIT ?1"))
        .map_err(db_err)?
        .query_map(params![limit], |r| map_info(dir, r))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)
}

/// Makes the table agree with the directory: forget backups whose file is gone, adopt files the table does
/// not know (e.g. made after the snapshot a restore came from) and remove leftovers of interrupted runs.
/// Returns (adopted, forgotten).
pub fn reconcile(conn: &Connection, dir: &Path, now: i64) -> (usize, usize) {
    let Ok(rd) = fs::read_dir(dir) else { return (0, 0) };
    let mut on_disk: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else { continue };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(now, |d| d.as_millis() as i64);
        if (name.ends_with(".part") || name.starts_with("tmp-")) && now - mtime > STALE_PART_MS {
            let _ = fs::remove_file(e.path());
        } else if safe_file_name(&name) && name.starts_with("exameow-backup-") && meta.is_file() {
            on_disk.insert(name, (meta.len() as i64, mtime));
        }
    }
    let known: Vec<(String, String)> = conn
        .prepare("SELECT id, file FROM backups WHERE ok = 1")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .unwrap_or_default();
    let mut forgotten = 0;
    for (id, file) in &known {
        if !on_disk.contains_key(file) {
            forgotten += conn.execute("DELETE FROM backups WHERE id = ?1", params![id]).unwrap_or(0);
        }
    }
    let mut adopted = 0;
    for (file, (size, mtime)) in on_disk {
        if known.iter().any(|(_, f)| *f == file) {
            continue;
        }
        let kind = if file.contains("-nightly") { "nightly" } else { "manual" };
        adopted += conn
            .execute(
                "INSERT INTO backups(id, file, kind, size, files_count, ok, created_at) VALUES (?1,?2,?3,?4,0,1,?5)",
                params![new_id(), file, kind, size, mtime],
            )
            .unwrap_or(0);
    }
    (adopted, forgotten)
}

// ───────── nightly schedule ─────────

/// The most recent scheduled slot (today's `hour`:00 UTC, or yesterday's if it has not come yet).
fn slot_start(now: i64, hour: u32) -> i64 {
    let day = now.div_euclid(86_400_000) * 86_400_000;
    let today = day + hour as i64 * 3_600_000;
    if now >= today { today } else { today - 86_400_000 }
}

/// Due when no nightly backup succeeded since the latest slot — so a server that was down at 03:00 backs up
/// at its first check afterwards — and the last failed attempt (if newer) is at least 3 hours old.
fn nightly_due(last_ok: Option<i64>, last_failed: Option<i64>, now: i64, hour: Option<u32>) -> bool {
    let Some(hour) = hour else { return false };
    if last_ok.map_or(false, |t| t >= slot_start(now, hour)) {
        return false;
    }
    !last_failed.map_or(false, |f| f > last_ok.unwrap_or(i64::MIN) && now - f < RETRY_AFTER_FAILURE_MS)
}

/// Entry point for the background loop: makes tonight's backup if it is due. Returns whether one was made.
pub fn run_nightly(state: &PlatformState, now: i64) -> bool {
    let (last_ok, last_failed) = match state.conn.lock() {
        Ok(conn) => {
            let at = |ok: i64| conn.query_row("SELECT max(created_at) FROM backups WHERE kind = 'nightly' AND ok = ?1", params![ok], |r| r.get::<_, Option<i64>>(0)).ok().flatten();
            (at(1), at(0))
        }
        Err(_) => return false,
    };
    if !nightly_due(last_ok, last_failed, now, state.backup.nightly_hour) {
        return false;
    }
    match create_backup(state, "nightly", None, now) {
        Ok(b) => {
            println!("Platform: nightly backup written ({} MiB, {} attachments)", b.size as u64 / MIB, b.files_count);
            true
        }
        Err(e) => {
            eprintln!("Platform: nightly backup failed: {}", e.1);
            false
        }
    }
}

/// Start-up housekeeping: sync the table with the directory.
pub fn startup(state: &PlatformState) {
    if let Ok(conn) = state.conn.lock() {
        let (adopted, forgotten) = reconcile(&conn, &state.layout.backup_dir, now_ms());
        if adopted + forgotten > 0 {
            println!("Platform: backups reconciled with {} (adopted {adopted}, forgot {forgotten})", state.layout.backup_dir.display());
        }
    }
}

// ───────── system status ─────────

#[derive(Serialize, Debug)]
pub struct SystemInfo {
    pub version: &'static str,
    pub uptime_sec: i64,
    pub db_bytes: u64,
    pub files_bytes: u64,
    pub files_count: u64,
    pub backups_bytes: u64,
    pub backups_count: i64,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub sessions_active: i64,
    pub users: i64,
    /// UTC hour of the nightly backup, or `None` when it is switched off.
    pub nightly_hour: Option<u32>,
    pub keep_nightly: usize,
    pub keep_manual: usize,
    pub backup_dir: String,
    pub last_backup: Option<BackupInfo>,
    pub last_failure: Option<BackupInfo>,
    pub warnings: Vec<&'static str>,
}

/// `low_disk` when under 500 MiB or 5 % of the volume is free.
fn low_disk(free: u64, total: u64) -> bool {
    free < LOW_DISK_BYTES || (total > 0 && free * 20 < total)
}

fn warnings(last_ok: Option<i64>, last_attempt_failed: bool, now: i64, nightly: bool, free: Option<(u64, u64)>) -> Vec<&'static str> {
    let mut w = Vec::new();
    match last_ok {
        None => w.push("no_backup"),
        Some(t) if now - t > if nightly { OVERDUE_NIGHTLY_MS } else { OVERDUE_MANUAL_ONLY_MS } => w.push("backup_overdue"),
        _ => {}
    }
    if last_attempt_failed {
        w.push("last_backup_failed");
    }
    if free.map_or(false, |(f, t)| low_disk(f, t)) {
        w.push("low_disk");
    }
    w
}

/// (latest good backup that still exists, whether a failure is newer than it) from newest-first rows.
fn summarize(list: &[BackupInfo]) -> (Option<&BackupInfo>, bool) {
    let last_ok = list.iter().find(|b| b.ok && b.present);
    let failed_since = match (list.iter().find(|b| !b.ok), last_ok) {
        (Some(f), Some(ok)) => f.created_at > ok.created_at,
        (Some(_), None) => true,
        _ => false,
    };
    (last_ok, failed_since)
}

/// The first volume (data or backups) that is short on space.
fn tight_disk(l: &Layout) -> Option<(u64, u64)> {
    [disk_space(&l.data_dir()), disk_space(&l.backup_dir)].into_iter().flatten().find(|(f, t)| low_disk(*f, *t))
}

pub fn system_info(state: &PlatformState, now: i64) -> Res<SystemInfo> {
    let l = &state.layout;
    let conn = state.conn.lock().map_err(db_err)?;
    let list = list_backups(&conn, &l.backup_dir, 200)?;
    let (last_ok, failed_since) = summarize(&list);
    let wal = fs::metadata(format!("{}-wal", l.db_path.display())).map(|m| m.len()).unwrap_or(0);
    let (files_bytes, files_count) = dir_stats(&l.files_dir);
    let good = || list.iter().filter(|b| b.ok && b.present);
    let data_disk = disk_space(&l.data_dir());
    let num = |sql: &str, p: &[&dyn rusqlite::ToSql]| conn.query_row(sql, p, |r| r.get::<_, i64>(0)).map_err(db_err);
    Ok(SystemInfo {
        version: VERSION,
        uptime_sec: ((now - state.started_at) / 1000).max(0),
        db_bytes: db_bytes(&conn) + wal,
        files_bytes,
        files_count,
        backups_bytes: good().map(|b| b.size.max(0) as u64).sum(),
        backups_count: good().count() as i64,
        disk_free_bytes: data_disk.map(|d| d.0),
        disk_total_bytes: data_disk.map(|d| d.1),
        sessions_active: num("SELECT count(*) FROM sessions WHERE expires_at > ?1", &[&now])?,
        users: num("SELECT count(*) FROM users", &[])?,
        nightly_hour: state.backup.nightly_hour,
        keep_nightly: state.backup.keep_nightly,
        keep_manual: state.backup.keep_manual,
        backup_dir: l.backup_dir.display().to_string(),
        warnings: warnings(last_ok.map(|b| b.created_at), failed_since, now, state.backup.nightly_hour.is_some(), tight_disk(l)),
        last_backup: last_ok.cloned(),
        last_failure: list.iter().find(|b| !b.ok).cloned(),
    })
}

/// Number of active warnings, for the admin sidebar badge (cheap: no directory walk).
pub fn warning_count(state: &PlatformState, now: i64) -> i64 {
    let Ok(conn) = state.conn.lock() else { return 0 };
    let list = list_backups(&conn, &state.layout.backup_dir, 30).unwrap_or_default();
    let (last_ok, failed_since) = summarize(&list);
    warnings(last_ok.map(|b| b.created_at), failed_since, now, state.backup.nightly_hour.is_some(), tight_disk(&state.layout)).len() as i64
}

// ───────── HTTP ─────────

pub async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<BackupInfo>> {
    let admin = require_admin(&s, &h)?;
    rate_limit(&*lock(&s)?, &format!("backup:{}", admin.id), 3_600_000, 6)?;
    let st = s.clone();
    let info = tokio::task::spawn_blocking(move || create_backup(&st.platform, "manual", Some(&admin.id), now_ms()))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "backup_failed"))??;
    Ok(Json(info))
}

pub async fn list_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<BackupInfo>>> {
    require_admin(&s, &h)?;
    list_backups(&*lock(&s)?, &s.platform.layout.backup_dir, 100).map(Json)
}

pub async fn system_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<SystemInfo>> {
    require_admin(&s, &h)?;
    let st = s.clone();
    tokio::task::spawn_blocking(move || system_info(&st.platform, now_ms()))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?
        .map(Json)
}

pub async fn download_handler(State(s): State<Arc<AppState>>, h: HeaderMap, AxPath(id): AxPath<String>) -> Result<Response, crate::relay::Err> {
    let admin = require_admin(&s, &h)?;
    let (name, size) = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("backup-dl:{}", admin.id), 60_000, 10)?;
        let b = get_info(&conn, &s.platform.layout.backup_dir, &id)?;
        if !b.ok || !b.present || !safe_file_name(&b.file) {
            return Err(err(StatusCode::NOT_FOUND, "not_found"));
        }
        audit(&conn, &admin.id, &id, "backup_downloaded", &b.kind);
        (b.file, b.size)
    };
    let file = tokio::fs::File::open(s.platform.layout.backup_dir.join(&name)).await.map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(size.max(0) as u64);
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

// ───────── restore (offline) ─────────

#[derive(Debug)]
pub struct RestoreReport {
    pub created_at: i64,
    pub kind: String,
    pub server_version: String,
    pub counts: BTreeMap<String, i64>,
    pub files: usize,
    pub has_key: bool,
    pub bytes: u64,
    /// Where the previous data was moved (only when the restore was applied).
    pub moved_aside: Option<PathBuf>,
}

/// An entry name from a backup is accepted only when it is exactly one of ours: the database, the key, or a
/// relative path under `files/` made of plain components.
fn entry_ok(name: &str) -> bool {
    if name == DB_ENTRY || name == KEY_ENTRY {
        return true;
    }
    let Some(rel) = name.strip_prefix(FILES_PREFIX) else { return false };
    !rel.is_empty() && !rel.contains('\\') && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Extracts and verifies `zip_path` into `staging` (SHA-256 of every entry, then SQLite integrity and row
/// counts). Nothing outside `staging` is touched.
fn stage(zip_path: &Path, staging: &Path) -> Result<(Manifest, u64), String> {
    let file = File::open(zip_path).map_err(|e| format!("cannot open {}: {e}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("not a valid backup archive: {e}"))?;
    let manifest: Manifest = {
        let mut m = archive.by_name(MANIFEST).map_err(|_| "backup has no manifest.json".to_string())?;
        let mut raw = String::new();
        m.read_to_string(&mut raw).map_err(|e| format!("manifest unreadable: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("manifest invalid: {e}"))?
    };
    if manifest.format != FORMAT || manifest.app != "exameow-platform" {
        return Err(format!("unsupported backup (format {}, app {})", manifest.format, manifest.app));
    }
    if !manifest.entries.iter().any(|e| e.name == DB_ENTRY) {
        return Err("backup contains no database".into());
    }
    let total: u64 = manifest.entries.iter().map(|e| e.size).sum();
    if let Some((free, _)) = disk_space(staging) {
        if free < total + 16 * MIB {
            return Err(format!("not enough free disk space to verify the backup ({} MiB needed)", (total + 16 * MIB) / MIB));
        }
    }
    for e in &manifest.entries {
        if !entry_ok(&e.name) {
            return Err(format!("backup contains an unsafe entry name: {}", e.name));
        }
        let dest = staging.join(&e.name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|x| x.to_string())?;
        }
        let mut src = archive.by_name(&e.name).map_err(|_| format!("entry missing from the archive: {}", e.name))?;
        let mut out = File::create(&dest).map_err(|x| x.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut n_total = 0u64;
        loop {
            let n = src.read(&mut buf).map_err(|x| format!("{}: {x}", e.name))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|x| x.to_string())?;
            n_total += n as u64;
            if n_total > e.size {
                // a hostile archive may claim a small size and stream far more: stop as soon as it overruns
                return Err(format!("checksum mismatch for {} — the backup is corrupted or was modified", e.name));
            }
        }
        if n_total != e.size || hex(&hasher.finalize()) != e.sha256 {
            return Err(format!("checksum mismatch for {} — the backup is corrupted or was modified", e.name));
        }
    }
    let db = Connection::open_with_flags(staging.join(DB_ENTRY), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| format!("database unreadable: {e}"))?;
    let check: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(|e| format!("integrity check failed: {e}"))?;
    if check != "ok" {
        return Err(format!("database integrity check failed: {check}"));
    }
    let counts = row_counts(&db);
    if counts != manifest.counts {
        return Err("row counts in the database differ from the manifest".into());
    }
    Ok((manifest, total))
}

/// Moves `from` to `to`, copying across file systems when a rename is not possible.
fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        copy_dir(from, to)?;
        fs::remove_dir_all(from)
    } else {
        fs::copy(from, to)?;
        fs::remove_file(from)
    }
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Verifies `zip_path` and, when `apply` is set, replaces the data described by `layout` with it. The previous
/// database (with its `-wal`/`-shm`), key and attachments are moved to `pre-restore-<stamp>/` next to the
/// database — never deleted. The server must not be running.
pub fn restore_from(zip_path: &Path, layout: &Layout, apply: bool, now: i64) -> Result<RestoreReport, String> {
    let data_dir = layout.data_dir();
    fs::create_dir_all(&data_dir).map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let staging = data_dir.join(format!(".restore-{}", stamp(now)));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("cannot create staging directory: {e}"))?;
    let cleanup = |r: Result<RestoreReport, String>| {
        let _ = fs::remove_dir_all(&staging);
        r
    };
    let (manifest, bytes) = match stage(zip_path, &staging) {
        Ok(v) => v,
        Err(e) => return cleanup(Err(e)),
    };
    let files = manifest.entries.iter().filter(|e| e.name.starts_with(FILES_PREFIX)).count();
    let has_key = manifest.entries.iter().any(|e| e.name == KEY_ENTRY);
    let mut report = RestoreReport { created_at: manifest.created_at, kind: manifest.kind, server_version: manifest.server_version, counts: manifest.counts, files, has_key, bytes, moved_aside: None };
    if !apply {
        return cleanup(Ok(report));
    }

    let aside = data_dir.join(format!("pre-restore-{}", stamp(now)));
    let wal = PathBuf::from(format!("{}-wal", layout.db_path.display()));
    let shm = PathBuf::from(format!("{}-shm", layout.db_path.display()));
    // (current location, parking place) for everything that will be replaced
    let mut parked: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut plan: Vec<(&Path, String)> = vec![(&layout.db_path, "db".into()), (&wal, "db-wal".into()), (&shm, "db-shm".into()), (&layout.files_dir, "files".into())];
    if has_key {
        plan.push((&layout.key_file, "key".into()));
    }
    let undo = |parked: &[(PathBuf, PathBuf)]| {
        for (orig, park) in parked.iter().rev() {
            let _ = fs::remove_file(orig);
            let _ = move_path(park, orig);
        }
    };
    if let Err(e) = fs::create_dir_all(&aside) {
        return cleanup(Err(format!("cannot create {}: {e}", aside.display())));
    }
    for (orig, name) in plan {
        if !orig.exists() {
            continue;
        }
        let park = aside.join(&name);
        if let Err(e) = move_path(orig, &park) {
            undo(&parked);
            let _ = fs::remove_dir_all(&aside);
            return cleanup(Err(format!("cannot move {} aside: {e}", orig.display())));
        }
        parked.push((orig.to_path_buf(), park));
    }
    let place = |from: PathBuf, to: &Path| move_path(&from, to).map_err(|e| format!("cannot restore {}: {e}", to.display()));
    let placed = (|| -> Result<(), String> {
        place(staging.join(DB_ENTRY), &layout.db_path)?;
        make_private(&layout.db_path);
        if has_key {
            place(staging.join(KEY_ENTRY), &layout.key_file)?;
            make_private(&layout.key_file);
        }
        let staged_files = staging.join("files");
        if staged_files.is_dir() {
            place(staged_files, &layout.files_dir)?;
        } else {
            fs::create_dir_all(&layout.files_dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = placed {
        // put the previous state back exactly as it was
        let _ = fs::remove_file(&layout.db_path);
        if has_key {
            let _ = fs::remove_file(&layout.key_file);
        }
        let _ = fs::remove_dir_all(&layout.files_dir);
        undo(&parked);
        let _ = fs::remove_dir_all(&aside);
        return cleanup(Err(format!("{e} — the previous data was put back")));
    }
    if parked.is_empty() {
        let _ = fs::remove_dir(&aside); // nothing existed before: no parking place needed
    } else {
        report.moved_aside = Some(aside);
    }
    cleanup(Ok(report))
}

/// `exameow-server restore <backup.zip> [--yes]` — verifies by default, replaces the data with `--yes`.
pub fn cli_restore(args: &[String]) -> i32 {
    let apply = args.iter().any(|a| a == "--yes" || a == "-y");
    let Some(file) = args.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("usage: exameow-server restore <backup.zip> [--yes]\n  without --yes the backup is only verified; with it the current data is replaced\n  (the server must be stopped; the old data is kept in pre-restore-<time>/)");
        return 2;
    };
    let layout = Layout::from_env();
    println!("Backup:   {file}\nDatabase: {}\nFiles:    {}\nKey file: {}", layout.db_path.display(), layout.files_dir.display(), layout.key_file.display());
    match restore_from(Path::new(file), &layout, apply, now_ms()) {
        Ok(r) => {
            let (y, m, d, h, mi, _) = civil_utc(r.created_at);
            println!(
                "Backup verified: made {y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC ({}, server {}), {} attachments, key {}, {} MiB.",
                r.kind, r.server_version, r.files, if r.has_key { "included" } else { "NOT included" }, r.bytes / MIB
            );
            println!("Rows: {}", r.counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
            match r.moved_aside {
                Some(p) => {
                    println!("RESTORED. The previous data was kept in {}. Start the server again.", p.display());
                    0
                }
                None => {
                    println!("Nothing was changed. Stop the server, then run again with --yes to restore this backup.");
                    0
                }
            }
        }
        Err(e) => {
            eprintln!("Restore failed: {e}\nNothing was changed.");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, User};
    use std::collections::BTreeSet;

    const DAY: i64 = 86_400_000;
    const HOUR: i64 = 3_600_000;
    /// 2001-09-09 01:46:40 UTC — a Sunday, so hour arithmetic below is easy to read.
    const NOW: i64 = 1_000_000_000_000;
    const MIDNIGHT: i64 = NOW - (NOW % DAY);

    struct T {
        state: PlatformState,
        admin: User,
        root: PathBuf,
    }

    impl Drop for T {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A platform with a few users, an encrypted setting, a key file and three attachments (one nested).
    fn t() -> T {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        insert_test_user(&conn, "t@x.com", "teacher", "active");
        insert_test_user(&conn, "s1@x.com", "student", "active");
        insert_test_user(&conn, "s2@x.com", "student", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        let state = PlatformState::for_tests(conn);
        crate::platform_settings::set(&state.conn.lock().unwrap(), &state.crypto, "ai.key", "sk-very-secret", true, None).unwrap();
        let l = state.layout.clone();
        write(&l.key_file, "07".repeat(32).as_bytes());
        write(&l.files_dir.join("a.pdf"), b"%PDF-1.4 first attachment");
        write(&l.files_dir.join("b.png"), &[0x89, b'P', b'N', b'G', 0, 1, 2, 3, 255]);
        write(&l.files_dir.join("sub/c.txt"), "نص عربي".as_bytes());
        let root = l.db_path.parent().unwrap().to_path_buf();
        T { state, admin, root }
    }

    fn zip_names(path: &Path) -> BTreeSet<String> {
        let mut a = ZipArchive::new(File::open(path).unwrap()).unwrap();
        (0..a.len()).map(|i| a.by_index(i).unwrap().name().to_string()).collect()
    }

    fn zip_bytes(path: &Path, name: &str) -> Vec<u8> {
        let mut a = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut v = Vec::new();
        a.by_name(name).unwrap().read_to_end(&mut v).unwrap();
        v
    }

    fn manifest_of(path: &Path) -> Manifest {
        serde_json::from_slice(&zip_bytes(path, MANIFEST)).unwrap()
    }

    fn backup_path(t: &T, info: &BackupInfo) -> PathBuf {
        t.state.layout.backup_dir.join(&info.file)
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    /// Rewrites an archive, letting `edit` change the manifest and/or entry bytes (to build hostile backups).
    fn tamper(src: &Path, dst: &Path, edit: impl Fn(&str, Vec<u8>) -> Option<(String, Vec<u8>)>) {
        let mut a = ZipArchive::new(File::open(src).unwrap()).unwrap();
        let mut zw = ZipWriter::new(File::create(dst).unwrap());
        for i in 0..a.len() {
            let mut e = a.by_index(i).unwrap();
            let mut data = Vec::new();
            e.read_to_end(&mut data).unwrap();
            if let Some((name, data)) = edit(e.name(), data) {
                zw.start_file(name, SimpleFileOptions::default()).unwrap();
                zw.write_all(&data).unwrap();
            }
        }
        zw.finish().unwrap();
    }

    fn edit_manifest(src: &Path, dst: &Path, f: impl Fn(&mut Manifest)) {
        tamper(src, dst, |name, data| {
            if name == MANIFEST {
                let mut m: Manifest = serde_json::from_slice(&data).unwrap();
                f(&mut m);
                Some((name.to_string(), serde_json::to_vec(&m).unwrap()))
            } else {
                Some((name.to_string(), data))
            }
        });
    }

    /// A second, independent platform root for restore targets, optionally already holding old data.
    fn target(with_data: bool) -> (Layout, PathBuf) {
        let root = std::env::temp_dir().join(format!("exameow-restore-{}", new_id()));
        let l = Layout { db_path: root.join("platform.db"), files_dir: root.join("files"), key_file: root.join("platform.key"), backup_dir: root.join("backups") };
        fs::create_dir_all(&root).unwrap();
        if with_data {
            let conn = Connection::open(&l.db_path).unwrap();
            crate::platform::apply_schema(&conn).unwrap();
            for i in 0..7 {
                insert_test_user(&conn, &format!("old{i}@x.com"), "student", "active");
            }
            write(&l.files_dir.join("old.txt"), b"old attachment");
            write(&l.key_file, "aa".repeat(32).as_bytes());
            write(&PathBuf::from(format!("{}-wal", l.db_path.display())), b"stale wal that must never be replayed");
        }
        (l, root)
    }

    fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
        list_files(dir).into_iter().map(|(rel, p)| (rel, fs::read(p).unwrap())).collect()
    }

    // ───────── creating backups ─────────

    #[test]
    fn a_backup_is_a_verifiable_archive_with_snapshot_key_attachments_and_manifest() {
        let t = t();
        let info = create_backup(&t.state, "manual", Some(&t.admin.id), NOW).unwrap();
        assert!(info.ok && info.present && info.kind == "manual" && info.size > 0 && info.files_count == 3);
        assert!(safe_file_name(&info.file) && info.file == "exameow-backup-20010909-014640-manual.zip", "{}", info.file);
        assert_eq!(info.created_by.as_deref(), Some("T"));
        let path = backup_path(&t, &info);
        assert_eq!(zip_names(&path), ["files/a.pdf", "files/b.png", "files/sub/c.txt", "manifest.json", "platform.db", "platform.key"].iter().map(|s| s.to_string()).collect());
        // every entry's checksum in the manifest matches the bytes actually stored
        let m = manifest_of(&path);
        assert_eq!((m.format, m.app.as_str(), m.kind.as_str(), m.created_at), (1, "exameow-platform", "manual", NOW));
        assert_eq!(m.entries.len(), 5);
        for e in &m.entries {
            let bytes = zip_bytes(&path, &e.name);
            assert_eq!((e.size, e.sha256.clone()), (bytes.len() as u64, sha(&bytes)), "{}", e.name);
        }
        assert_eq!(zip_bytes(&path, KEY_ENTRY), "07".repeat(32).as_bytes(), "the master key travels with the backup");
        assert_eq!(zip_bytes(&path, "files/sub/c.txt"), "نص عربي".as_bytes());
        // the database inside is a real, intact SQLite file holding the same rows
        let db_file = t.root.join("extracted.db");
        fs::write(&db_file, zip_bytes(&path, DB_ENTRY)).unwrap();
        let db = Connection::open(&db_file).unwrap();
        assert_eq!(db.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0)).unwrap(), "ok");
        assert_eq!((count(&db, "users"), count(&db, "subjects"), count(&db, "settings")), (4, 1, 1));
        assert_eq!(m.counts["users"], 4);
        // the secret is stored encrypted, never in the clear, in the snapshot
        let stored: String = db.query_row("SELECT value FROM settings WHERE key = 'ai.key'", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains("sk-very-secret") && stored.starts_with("v1."));
        // nothing is left behind and the file is private
        let leftovers: Vec<String> = fs::read_dir(&t.state.layout.backup_dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".part") || n.starts_with("tmp-")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "the archive holds the key");
            assert_eq!(fs::metadata(&t.state.layout.backup_dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let conn = t.state.conn.lock().unwrap();
        assert_eq!(conn.query_row::<i64, _, _>("SELECT count(*) FROM audit_log WHERE action = 'backup_created' AND detail = 'manual'", [], |r| r.get(0)).unwrap(), 1);
    }

    #[test]
    fn the_snapshot_holds_exactly_what_was_committed_when_it_was_taken() {
        let t = t();
        let first = create_backup(&t.state, "manual", None, NOW).unwrap();
        insert_test_user(&t.state.conn.lock().unwrap(), "late@x.com", "student", "active");
        let second = create_backup(&t.state, "manual", None, NOW + 5_000).unwrap();
        let users_in = |info: &BackupInfo| manifest_of(&backup_path(&t, info)).counts["users"];
        assert_eq!((users_in(&first), users_in(&second)), (4, 5));
    }

    #[test]
    fn a_missing_key_file_or_attachment_directory_still_gives_a_valid_backup() {
        let t = t();
        fs::remove_file(&t.state.layout.key_file).unwrap();
        fs::remove_dir_all(&t.state.layout.files_dir).unwrap();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        assert_eq!(zip_names(&backup_path(&t, &info)), ["manifest.json", "platform.db"].iter().map(|s| s.to_string()).collect());
        assert_eq!(info.files_count, 0);
    }

    #[test]
    fn only_one_backup_runs_at_a_time() {
        let t = t();
        t.state.backup_busy.store(true, Ordering::SeqCst);
        assert_eq!(create_backup(&t.state, "manual", None, NOW).unwrap_err().0, StatusCode::CONFLICT);
        assert!(t.state.backup_busy.load(Ordering::SeqCst), "a refused request must not release someone else's lock");
        t.state.backup_busy.store(false, Ordering::SeqCst);
        create_backup(&t.state, "manual", None, NOW).unwrap();
        assert!(!t.state.backup_busy.load(Ordering::SeqCst), "released after a normal run");
        assert_eq!(create_backup(&t.state, "weekly", None, NOW).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_failed_backup_is_recorded_cleaned_up_and_reported_until_one_succeeds() {
        let mut t = t();
        let good_dir = t.state.layout.backup_dir.clone();
        // the backup directory cannot be created: its parent is a regular file
        write(&t.root.join("blocker"), b"x");
        t.state.layout.backup_dir = t.root.join("blocker/backups");
        let e = create_backup(&t.state, "manual", Some(&t.admin.id), NOW).unwrap_err();
        assert_eq!((e.0, e.1.contains("backup_failed")), (StatusCode::INTERNAL_SERVER_ERROR, true));
        assert!(!t.state.backup_busy.load(Ordering::SeqCst), "the lock is released after a failure");
        let info = system_info(&t.state, NOW + 1).unwrap();
        let w: Vec<_> = info.warnings.iter().filter(|w| **w != "low_disk").collect();
        assert_eq!(w, [&"no_backup", &"last_backup_failed"]);
        assert!(info.last_failure.as_ref().map_or(false, |f| !f.ok && f.error.as_deref().unwrap_or("").starts_with("backup_failed: ")));
        // a later success clears the failure warning
        t.state.layout.backup_dir = good_dir;
        create_backup(&t.state, "manual", None, NOW + 10_000).unwrap();
        let w: Vec<_> = system_info(&t.state, NOW + 20_000).unwrap().warnings.into_iter().filter(|w| *w != "low_disk").collect();
        assert!(w.is_empty(), "{w:?}");
        // failure records older than 30 days are forgotten by the next backup
        create_backup(&t.state, "manual", None, NOW + 31 * DAY).unwrap();
        let conn = t.state.conn.lock().unwrap();
        assert_eq!(count(&conn, "backups WHERE ok = 0"), 0);
    }

    #[test]
    fn retention_keeps_the_newest_per_kind_independently() {
        let mut t = t();
        t.state.backup = BackupConfig { keep_nightly: 3, keep_manual: 2, nightly_hour: Some(3) };
        for i in 0..5 {
            create_backup(&t.state, "nightly", None, NOW + i * DAY).unwrap();
        }
        for i in 0..4 {
            create_backup(&t.state, "manual", Some(&t.admin.id), NOW + i * DAY + HOUR).unwrap();
        }
        let list = list_backups(&t.state.conn.lock().unwrap(), &t.state.layout.backup_dir, 100).unwrap();
        let of = |k: &str| list.iter().filter(|b| b.kind == k).map(|b| b.created_at).collect::<Vec<_>>();
        assert_eq!(of("nightly"), vec![NOW + 4 * DAY, NOW + 3 * DAY, NOW + 2 * DAY], "newest 3 nightly");
        assert_eq!(of("manual"), vec![NOW + 3 * DAY + HOUR, NOW + 2 * DAY + HOUR], "newest 2 manual — manual ones never evict nightly ones");
        assert!(list.iter().all(|b| b.present));
        let files = fs::read_dir(&t.state.layout.backup_dir).unwrap().count();
        assert_eq!(files, 5, "evicted backups are removed from disk too");
    }

    #[test]
    fn nightly_slots_compensate_for_downtime_and_back_off_after_failures() {
        let h = Some(3);
        let at = |day: i64, hour: i64, min: i64| MIDNIGHT + day * DAY + hour * HOUR + min * 60_000;
        // before tonight's slot the previous night's backup is enough
        assert!(!nightly_due(Some(at(-1, 3, 10)), None, at(0, 2, 59), h));
        // after the slot, no backup since → due; with one made just after the slot → not due
        assert!(nightly_due(Some(at(-1, 3, 10)), None, at(0, 3, 0), h));
        assert!(!nightly_due(Some(at(0, 3, 5)), None, at(0, 23, 0), h));
        // never backed up → due at once; switched off → never
        assert!(nightly_due(None, None, at(0, 12, 0), h));
        assert!(!nightly_due(None, None, at(0, 12, 0), None));
        // the server was down for days: one backup at the first check, not one per missed night
        assert!(nightly_due(Some(at(-3, 3, 10)), None, at(0, 9, 0), h));
        // a failure blocks retries for 3 hours, but only if it is newer than the last success
        assert!(!nightly_due(Some(at(-1, 3, 10)), Some(at(0, 3, 30)), at(0, 5, 0), h));
        assert!(nightly_due(Some(at(-1, 3, 10)), Some(at(0, 3, 30)), at(0, 6, 31), h));
        assert!(nightly_due(Some(at(0, 3, 40)), Some(at(0, 3, 30)), at(1, 3, 0), h), "an old failure is irrelevant");
        assert_eq!((slot_start(at(0, 2, 0), 3), slot_start(at(0, 3, 0), 3), slot_start(at(0, 23, 59), 3)), (at(-1, 3, 0), at(0, 3, 0), at(0, 3, 0)));
        assert_eq!(slot_start(at(0, 0, 0), 0), at(0, 0, 0));
    }

    #[test]
    fn the_nightly_job_makes_one_backup_per_night() {
        let t = t();
        let at = |day: i64, hour: i64, min: i64| MIDNIGHT + day * DAY + hour * HOUR + min * 60_000;
        assert!(!run_nightly(&t.state, at(0, 3, 0)) == false, "first run: due");
        assert!(!run_nightly(&t.state, at(0, 4, 0)), "same night: nothing more");
        assert!(!run_nightly(&t.state, at(1, 2, 0)), "before the next slot");
        assert!(run_nightly(&t.state, at(1, 3, 5)), "next night");
        let conn = t.state.conn.lock().unwrap();
        assert_eq!(count(&conn, "backups WHERE kind = 'nightly' AND ok = 1"), 2);
        assert_eq!(conn.query_row::<i64, _, _>("SELECT count(*) FROM backups WHERE created_by IS NOT NULL", [], |r| r.get(0)).unwrap(), 0, "nightly backups have no author");
        drop(conn);
        let mut off = t;
        off.state.backup.nightly_hour = None;
        assert!(!run_nightly(&off.state, at(5, 12, 0)), "switched off");
    }

    #[test]
    fn a_failing_nightly_job_is_not_retried_every_hour() {
        let mut t = t();
        write(&t.root.join("blocker"), b"x");
        t.state.layout.backup_dir = t.root.join("blocker/backups");
        let at = |hour: i64| MIDNIGHT + DAY + hour * HOUR;
        assert!(!run_nightly(&t.state, at(3)));
        assert!(!run_nightly(&t.state, at(4)));
        assert!(!run_nightly(&t.state, at(5)));
        let failures = || count(&t.state.conn.lock().unwrap(), "backups WHERE ok = 0");
        assert_eq!(failures(), 1, "one attempt within the 3-hour back-off");
        assert!(!run_nightly(&t.state, at(7)));
        assert_eq!(failures(), 2, "retried after the back-off");
    }

    // ───────── directory reconciliation ─────────

    #[test]
    fn the_backup_directory_is_the_source_of_truth() {
        let t = t();
        let a = create_backup(&t.state, "nightly", None, NOW).unwrap();
        let b = create_backup(&t.state, "manual", Some(&t.admin.id), NOW + 10_000).unwrap();
        let dir = t.state.layout.backup_dir.clone();
        // as after restoring an older snapshot: the table knows a backup whose file is gone and lacks a newer one
        fs::remove_file(dir.join(&a.file)).unwrap();
        t.state.conn.lock().unwrap().execute("DELETE FROM backups WHERE id = ?1", params![b.id]).unwrap();
        // leftovers of an interrupted run, and a file that is none of our business
        write(&dir.join("exameow-backup-x.zip.part"), b"partial");
        write(&dir.join("tmp-123.db"), b"snapshot");
        write(&dir.join("notes.txt"), b"keep me");
        write(&dir.join("fresh.zip.part"), b"in progress right now");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600);
        for n in ["exameow-backup-x.zip.part", "tmp-123.db"] {
            File::options().write(true).open(dir.join(n)).unwrap().set_modified(old).unwrap();
        }
        let conn = t.state.conn.lock().unwrap();
        let now = now_ms();
        assert_eq!(reconcile(&conn, &dir, now), (1, 1));
        let list = list_backups(&conn, &dir, 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].file.as_str(), list[0].kind.as_str(), list[0].ok, list[0].present), (b.file.as_str(), "manual", true, true));
        assert_eq!(list[0].size, fs::metadata(dir.join(&b.file)).unwrap().len() as i64);
        assert!(!dir.join("exameow-backup-x.zip.part").exists() && !dir.join("tmp-123.db").exists(), "stale leftovers are removed");
        assert!(dir.join("notes.txt").exists() && dir.join("fresh.zip.part").exists(), "unrelated and recent files stay");
        assert_eq!(reconcile(&conn, &dir, now), (0, 0), "idempotent");
    }

    // ───────── restore ─────────

    #[test]
    fn restoring_a_backup_reproduces_the_data_the_attachments_and_the_secrets() {
        let t = t();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        let (dst, root) = target(false);
        let report = restore_from(&backup_path(&t, &info), &dst, true, NOW + 1000).unwrap();
        assert!(report.has_key && report.files == 3 && report.moved_aside.is_none(), "nothing existed, so nothing was moved aside");
        // same rows everywhere
        let restored = Connection::open(&dst.db_path).unwrap();
        assert_eq!(row_counts(&restored), report.counts);
        // …which is the live data as of the snapshot (the live audit log has since gained the "backup_created" entry)
        let mut live = row_counts(&t.state.conn.lock().unwrap());
        assert_eq!(live.remove("audit_log"), Some(1));
        let mut as_restored = row_counts(&restored);
        assert_eq!(as_restored.remove("audit_log"), Some(0));
        assert_eq!(as_restored, live);
        assert_eq!(restored.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0)).unwrap(), "ok");
        // identical attachments and key
        assert_eq!(tree(&dst.files_dir), tree(&t.state.layout.files_dir));
        assert_eq!(fs::read(&dst.key_file).unwrap(), fs::read(&t.state.layout.key_file).unwrap());
        // the encrypted secret opens with the restored key
        let crypto = crate::platform_settings::Crypto::from_key_file(&dst.key_file).unwrap();
        assert_eq!(crate::platform_settings::get(&restored, &crypto, "ai.key").unwrap().as_deref(), Some("sk-very-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&dst.key_file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(!fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".restore-")), "no staging leftovers");
        // and the restored database is a fully working platform: the server opens it and keeps its data
        let reopened = crate::platform::init_db(&dst).unwrap();
        assert_eq!(count(&reopened.conn.lock().unwrap(), "users"), 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verifying_without_applying_changes_nothing() {
        let t = t();
        let info = create_backup(&t.state, "nightly", None, NOW).unwrap();
        let (dst, root) = target(true);
        let before = (fs::read(&dst.db_path).unwrap(), tree(&dst.files_dir), fs::read(&dst.key_file).unwrap());
        let report = restore_from(&backup_path(&t, &info), &dst, false, NOW).unwrap();
        assert_eq!((report.kind.as_str(), report.files, report.counts["users"]), ("nightly", 3, 4));
        assert_eq!(before, (fs::read(&dst.db_path).unwrap(), tree(&dst.files_dir), fs::read(&dst.key_file).unwrap()));
        let names: Vec<String> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.starts_with("pre-restore") || n.starts_with(".restore")), "{names:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restore_over_existing_data_keeps_the_old_copy_and_never_replays_a_stale_wal() {
        let t = t();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        let (dst, root) = target(true);
        let wal = PathBuf::from(format!("{}-wal", dst.db_path.display()));
        assert!(wal.exists());
        let report = restore_from(&backup_path(&t, &info), &dst, true, NOW + 5000).unwrap();
        let aside = report.moved_aside.expect("old data parked");
        assert!(aside.ends_with("pre-restore-20010909-014645"), "{}", aside.display());
        // the live paths now hold the backup
        assert_eq!(count(&Connection::open(&dst.db_path).unwrap(), "users"), 4);
        assert!(!wal.exists(), "the old write-ahead log must not sit next to the restored database");
        assert_eq!(tree(&dst.files_dir), tree(&t.state.layout.files_dir));
        assert_eq!(fs::read(&dst.key_file).unwrap(), "07".repeat(32).as_bytes());
        // the previous state is intact under pre-restore-*
        // (read the log first: opening `db` would make SQLite consume its sibling `db-wal`)
        assert_eq!(fs::read(aside.join("db-wal")).unwrap(), b"stale wal that must never be replayed");
        assert_eq!(fs::read(aside.join("files/old.txt")).unwrap(), b"old attachment");
        assert_eq!(fs::read(aside.join("key")).unwrap(), "aa".repeat(32).as_bytes());
        fs::remove_file(aside.join("db-wal")).unwrap();
        assert_eq!(count(&Connection::open(aside.join("db")).unwrap(), "users"), 7);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_or_hostile_archives_are_refused_and_nothing_is_touched() {
        let t = t();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        let good = backup_path(&t, &info);
        let (dst, root) = target(true);
        let snapshot = || (fs::read(&dst.db_path).unwrap(), tree(&dst.files_dir), fs::read(&dst.key_file).unwrap(), fs::read_dir(&root).unwrap().count());
        let before = snapshot();
        let bad = root.join("bad.zip");
        let cases: Vec<(&str, Box<dyn Fn(&Path)>, &str)> = vec![
            ("flipped byte in the database", Box::new(|p| tamper(&good, p, |n, mut d| { if n == DB_ENTRY { let i = d.len() / 2; d[i] ^= 0xff; } Some((n.into(), d)) })), "checksum mismatch"),
            ("modified attachment", Box::new(|p| tamper(&good, p, |n, mut d| { if n == "files/a.pdf" { d.push(b'!'); } Some((n.into(), d)) })), "checksum mismatch"),
            ("modified key", Box::new(|p| tamper(&good, p, |n, d| Some((n.into(), if n == KEY_ENTRY { b"00".repeat(32) } else { d })))), "checksum mismatch"),
            ("no manifest", Box::new(|p| tamper(&good, p, |n, d| (n != MANIFEST).then(|| (n.into(), d)))), "manifest"),
            ("entry listed but missing", Box::new(|p| tamper(&good, p, |n, d| (n != "files/b.png").then(|| (n.into(), d)))), "missing"),
            ("future format", Box::new(|p| edit_manifest(&good, p, |m| m.format = 2)), "unsupported"),
            ("foreign app", Box::new(|p| edit_manifest(&good, p, |m| m.app = "something-else".into())), "unsupported"),
            ("counts that do not match the data", Box::new(|p| edit_manifest(&good, p, |m| { m.counts.insert("users".into(), 999); })), "row counts"),
            ("path traversal", Box::new(|p| edit_manifest(&good, p, |m| m.entries.push(EntryMeta { name: "files/../../evil.txt".into(), size: 0, sha256: sha(b"") }))), "unsafe entry"),
            ("absolute path", Box::new(|p| edit_manifest(&good, p, |m| m.entries.push(EntryMeta { name: "files//etc/passwd".into(), size: 0, sha256: sha(b"") }))), "unsafe entry"),
            ("unknown top-level entry", Box::new(|p| edit_manifest(&good, p, |m| m.entries.push(EntryMeta { name: "../outside".into(), size: 0, sha256: sha(b"") }))), "unsafe entry"),
            ("no database", Box::new(|p| edit_manifest(&good, p, |m| m.entries.retain(|e| e.name != DB_ENTRY))), "no database"),
            ("not an archive", Box::new(|p| fs::write(p, b"definitely not a zip file").unwrap()), "not a valid backup"),
            // a database that fails SQLite's own integrity check even though its checksum was recomputed to match
            ("internally inconsistent database", Box::new(|p| {
                let tmp = root.join("inconsistent.db");
                fs::write(&tmp, zip_bytes(&good, DB_ENTRY)).unwrap();
                // orphan a table's pages: the schema forgets `junk`, so SQLite's integrity check finds unreferenced pages
                Connection::open(&tmp).unwrap().execute_batch("CREATE TABLE junk(x); INSERT INTO junk VALUES (randomblob(2000)), (randomblob(2000)); PRAGMA writable_schema = ON; DELETE FROM sqlite_master WHERE name = 'junk';").unwrap();
                let bytes = fs::read(&tmp).unwrap();
                tamper(&good, p, |n, d| match n {
                    DB_ENTRY => Some((n.into(), bytes.clone())),
                    MANIFEST => {
                        let mut m: Manifest = serde_json::from_slice(&d).unwrap();
                        m.entries.iter_mut().filter(|e| e.name == DB_ENTRY).for_each(|e| { e.size = bytes.len() as u64; e.sha256 = sha(&bytes); });
                        Some((n.into(), serde_json::to_vec(&m).unwrap()))
                    }
                    _ => Some((n.into(), d)),
                });
                fs::remove_file(&tmp).unwrap();
            }), "integrity check failed"),
        ];
        for (label, make, expect) in cases {
            make(&bad);
            let e = restore_from(&bad, &dst, true, NOW + 9000).unwrap_err();
            assert!(e.contains(expect), "{label}: {e}");
            // the refused archive left no trace: same database, attachments, key; only bad.zip itself in the folder
            let after = snapshot();
            assert_eq!((&before.0, &before.1, &before.2), (&after.0, &after.1, &after.2), "{label}: data changed");
            assert_eq!(after.3, before.3 + 1, "{label}: staging or parking directories left behind");
        }
        assert!(restore_from(&root.join("nope.zip"), &dst, true, NOW).unwrap_err().contains("cannot open"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restore_that_fails_halfway_puts_the_previous_data_back() {
        let t = t();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        let (mut dst, root) = target(true);
        let before = (fs::read(&dst.db_path).unwrap(), fs::read(&dst.key_file).unwrap(), fs::read(PathBuf::from(format!("{}-wal", dst.db_path.display()))).unwrap());
        // the attachment directory can neither exist nor be created: its parent is a regular file
        write(&root.join("blocker"), b"x");
        dst.files_dir = root.join("blocker/files");
        let e = restore_from(&backup_path(&t, &info), &dst, true, NOW + 7000).unwrap_err();
        assert!(e.contains("previous data was put back"), "{e}");
        assert_eq!(before, (fs::read(&dst.db_path).unwrap(), fs::read(&dst.key_file).unwrap(), fs::read(PathBuf::from(format!("{}-wal", dst.db_path.display()))).unwrap()));
        assert_eq!(count(&Connection::open(&dst.db_path).unwrap(), "users"), 7);
        let names: Vec<String> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.starts_with("pre-restore") || n.starts_with(".restore")), "{names:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_backup_made_without_a_key_restores_without_touching_the_current_key() {
        let t = t();
        fs::remove_file(&t.state.layout.key_file).unwrap();
        let info = create_backup(&t.state, "manual", None, NOW).unwrap();
        let (dst, root) = target(true);
        let report = restore_from(&backup_path(&t, &info), &dst, true, NOW + 2000).unwrap();
        assert!(!report.has_key);
        assert_eq!(fs::read(&dst.key_file).unwrap(), "aa".repeat(32).as_bytes(), "the key already on the server stays");
        let _ = fs::remove_dir_all(&root);
    }

    // ───────── system status & small rules ─────────

    #[test]
    fn system_status_reports_sizes_sessions_and_backup_health() {
        let mut t = t();
        t.state.started_at = NOW;
        {
            let conn = t.state.conn.lock().unwrap();
            let uid: String = conn.query_row("SELECT id FROM users LIMIT 1", [], |r| r.get(0)).unwrap();
            conn.execute("INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES ('h1', ?1, 0, ?2), ('h2', ?1, 0, ?3)", params![uid, NOW + DAY, NOW - 1]).unwrap();
        }
        let strip = |mut w: Vec<&'static str>| { w.retain(|x| *x != "low_disk"); w };
        let info = system_info(&t.state, NOW).unwrap();
        assert_eq!((info.users, info.sessions_active, info.files_count, info.backups_count), (4, 1, 3, 0), "expired sessions are not counted");
        assert_eq!(info.files_bytes, ("%PDF-1.4 first attachment".len() + 9 + "نص عربي".len()) as u64);
        assert!(info.db_bytes > 0 && info.last_backup.is_none() && info.version == VERSION && info.nightly_hour == Some(3));
        assert_eq!(strip(info.warnings), vec!["no_backup"]);
        #[cfg(unix)]
        assert!(info.disk_free_bytes.is_some() && info.disk_total_bytes.is_some());
        let made = create_backup(&t.state, "manual", None, NOW).unwrap();
        let info = system_info(&t.state, NOW + 1000).unwrap();
        assert_eq!((info.backups_count, info.backups_bytes as i64, info.last_backup.map(|b| b.id)), (1, made.size, Some(made.id.clone())));
        assert_eq!((info.uptime_sec, strip(info.warnings)), (1, vec![]));
        // overdue: 36 h with a nightly job, 7 days without
        assert_eq!(strip(system_info(&t.state, NOW + 37 * HOUR).unwrap().warnings), vec!["backup_overdue"]);
        let mut manual_only = t;
        manual_only.state.backup.nightly_hour = None;
        assert!(strip(system_info(&manual_only.state, NOW + 37 * HOUR).unwrap().warnings).is_empty());
        assert_eq!(strip(system_info(&manual_only.state, NOW + 8 * DAY).unwrap().warnings), vec!["backup_overdue"]);
        // a backup file deleted by hand no longer counts as a backup
        fs::remove_file(manual_only.state.layout.backup_dir.join(&made.file)).unwrap();
        let info = system_info(&manual_only.state, NOW + 2000).unwrap();
        let low = info.warnings.iter().filter(|w| **w == "low_disk").count() as i64;
        assert_eq!((info.backups_count, strip(info.warnings)), (0, vec!["no_backup"]));
        assert_eq!(warning_count(&manual_only.state, NOW + 2000), 1 + low, "the sidebar badge counts the same warnings");
    }

    #[test]
    fn warning_and_disk_rules() {
        assert!(low_disk(499 * MIB, 1 << 40) && !low_disk(501 * MIB, 8 * (1 << 30)));
        assert!(low_disk(2 << 30, 100 << 30), "under 5 % free");
        assert!(!low_disk(6 << 30, 100 << 30));
        assert_eq!(warnings(None, false, NOW, true, None), vec!["no_backup"]);
        assert_eq!(warnings(Some(NOW), true, NOW + 1, true, Some((1, 100))), vec!["last_backup_failed", "low_disk"]);
        assert_eq!(warnings(Some(NOW - 35 * HOUR), false, NOW, true, None), Vec::<&str>::new());
        assert_eq!(warnings(Some(NOW - 37 * HOUR), false, NOW, true, None), vec!["backup_overdue"]);
        assert_eq!(required_bytes(10 * MIB, 100 * MIB), 128 * MIB);
    }

    #[test]
    fn file_names_and_archive_entries_are_validated_before_use() {
        for good in ["exameow-backup-20010909-014640-manual.zip", "a_b-1.zip"] {
            assert!(safe_file_name(good), "{good}");
        }
        for bad in ["", "..zip", "../x.zip", "a/b.zip", "a\\b.zip", ".hidden.zip", "x.zip.part", "x.tar", "a b.zip", "نسخة.zip", "a..b.zip", &"a".repeat(130)] {
            assert!(!safe_file_name(bad), "{bad}");
        }
        for good in ["platform.db", "platform.key", "files/a.pdf", "files/sub/c.txt"] {
            assert!(entry_ok(good), "{good}");
        }
        for bad in ["manifest.json", "files/", "files", "files/../x", "files//etc/passwd", "/platform.db", "files/a\\b", "other/x", "../platform.db", "platform.db/../x"] {
            assert!(!entry_ok(bad), "{bad}");
        }
    }

    #[test]
    fn configuration_parsing_never_switches_the_safety_net_off_by_accident() {
        let d = BackupConfig::default();
        assert_eq!((d.keep_nightly, d.keep_manual, d.nightly_hour), (7, 5, Some(3)));
        let p = |h, k, m| { let c = BackupConfig::parse(h, k, m); (c.nightly_hour, c.keep_nightly, c.keep_manual) };
        assert_eq!(p(None, None, None), (Some(3), 7, 5));
        assert_eq!(p(Some("off"), None, None), (None, 7, 5));
        assert_eq!(p(Some(" OFF "), Some(" 14 "), Some("2")), (None, 14, 2));
        assert_eq!(p(Some("0"), None, None), (Some(0), 7, 5));
        assert_eq!(p(Some("23"), None, None), (Some(23), 7, 5));
        assert_eq!(p(Some("24"), Some("0"), Some("-1")), (Some(3), 7, 5), "out of range → defaults");
        assert_eq!(p(Some("night"), Some("many"), Some("")), (Some(3), 7, 5));
        assert_eq!(p(None, Some("9999"), None), (Some(3), 7, 5));
    }

    #[test]
    fn dates_and_paths_are_computed_without_surprises() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(NOW), "20010909-014640");
        assert_eq!(stamp(1_709_164_799_000), "20240228-235959");
        assert_eq!(stamp(1_709_164_800_000), "20240229-000000", "leap day");
        assert_eq!(stamp(1_709_164_800_000 + DAY), "20240301-000000");
        assert_eq!(data_dir_of(Path::new("./x.db")), PathBuf::from("."));
        assert_eq!(data_dir_of(Path::new("x.db")), PathBuf::from("."));
        assert_eq!(data_dir_of(Path::new("/data/x.db")), PathBuf::from("/data"));
    }
}
