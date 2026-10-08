//! Server-side question bank (admin only): reusable questions that belong to a subject, with search,
//! filters, archive/delete, bulk import and a soft duplicate warning. Exams copy questions as a
//! snapshot when they are created (phase 1-5), so editing or deleting a bank item never changes an
//! exam that already exists.

use crate::platform::{audit, bad, db_err, lock, new_id, require_admin, Res, User};
use crate::platform_exams::{norm_text, validate_question};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use exameow_core::exam::{Difficulty, Question, QuestionType};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS question_bank_items (
  id TEXT PRIMARY KEY,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  type TEXT NOT NULL CHECK (type IN ('single_choice','multi_choice','true_false','fill_blank','short_answer')),
  stem TEXT NOT NULL,
  stem_norm TEXT NOT NULL,              -- Arabic-folded, whitespace-collapsed: duplicate check + search
  options TEXT NOT NULL DEFAULT '[]',
  answer TEXT NOT NULL,
  analysis TEXT NOT NULL DEFAULT '',
  chapter TEXT,
  difficulty TEXT CHECK (difficulty IS NULL OR difficulty IN ('easy','medium','hard')),
  tags TEXT NOT NULL DEFAULT '[]',
  created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_bank_subject ON question_bank_items(subject_id, archived_at, created_at);
CREATE INDEX IF NOT EXISTS idx_bank_norm ON question_bank_items(subject_id, stem_norm);
";

const MAX_IMPORT: usize = 500;
const MAX_BULK: usize = 500;
const MAX_PER_SUBJECT: i64 = 20_000;
const MAX_TAGS: usize = 10;
const MAX_TAG_CHARS: usize = 30;
const MAX_CHAPTER_CHARS: usize = 100;
const DEFAULT_PAGE: i64 = 25;
const MAX_PAGE: i64 = 100;

// ───────── normalisation ─────────

/// Comparison form of a stem: Arabic-folded (see `norm_text`) with whitespace collapsed.
pub fn stem_norm(s: &str) -> String {
    norm_text(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

const TRUE_WORDS: &[&str] = &["a", "true", "t", "yes", "y", "1", "√", "对", "正确", "是", "صح", "صحيح", "نعم"];
const FALSE_WORDS: &[&str] = &["b", "false", "f", "no", "n", "0", "×", "错", "错误", "否", "خطا", "خاطي", "خاطئ", "خطاء", "لا", "غير صحيح"];

/// Brings an answer into the form the graders expect: choice letters sorted (`AC`), true/false as `A`/`B`.
fn normalize_answer(qtype: &QuestionType, options: &[String], answer: &str) -> Res<String> {
    match qtype {
        QuestionType::SingleChoice | QuestionType::MultiChoice => {
            // Leading letters/separators only, so "B. القاهرة" and "A, C" both work; a letter
            // beyond the option count (e.g. from plain prose) is rejected below.
            let mut letters: Vec<char> = answer
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphabetic() || matches!(c, ',' | '،' | ';' | '؛' | '、' | '/' | '&' | '+' | '-' | ' '))
                .filter(char::is_ascii_alphabetic)
                .map(|c| c.to_ascii_uppercase())
                .collect();
            letters.sort();
            letters.dedup();
            let ok = !letters.is_empty() && letters.iter().all(|c| ((*c as u8 - b'A') as usize) < options.len());
            if !ok || (*qtype == QuestionType::SingleChoice && letters.len() != 1) {
                return Err(bad("invalid_answer"));
            }
            Ok(letters.into_iter().collect())
        }
        QuestionType::TrueFalse => {
            let n = norm_text(answer);
            if TRUE_WORDS.contains(&n.as_str()) {
                Ok("A".into())
            } else if FALSE_WORDS.contains(&n.as_str()) {
                Ok("B".into())
            } else {
                Err(bad("invalid_answer"))
            }
        }
        QuestionType::FillBlank | QuestionType::ShortAnswer => Ok(answer.trim().to_string()),
    }
}

fn clean_tags(tags: &[String]) -> Res<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for t in tags {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > MAX_TAG_CHARS || t.chars().any(char::is_control) {
            return Err(bad("invalid_tags"));
        }
        if !out.iter().any(|x| norm_text(x) == norm_text(t)) {
            out.push(t.to_string());
        }
    }
    if out.len() > MAX_TAGS {
        return Err(bad("invalid_tags"));
    }
    Ok(out)
}

// ───────── data types ─────────

#[derive(Deserialize, Clone)]
pub struct ItemIn {
    #[serde(rename = "type")]
    qtype: QuestionType,
    stem: String,
    #[serde(default)]
    options: Vec<String>,
    answer: String,
    #[serde(default)]
    analysis: String,
    chapter: Option<String>,
    difficulty: Option<Difficulty>,
    #[serde(default)]
    tags: Vec<String>,
}

/// A validated, normalised item ready to store.
struct Clean {
    qtype: String,
    stem: String,
    norm: String,
    options: Vec<String>,
    answer: String,
    analysis: String,
    chapter: Option<String>,
    difficulty: Option<String>,
    tags: Vec<String>,
}

fn clean(i: &ItemIn) -> Res<Clean> {
    let mut options: Vec<String> = i.options.iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
    if i.qtype == QuestionType::TrueFalse && options.is_empty() {
        options = vec!["صحيح".into(), "خطأ".into()];
    }
    if !matches!(i.qtype, QuestionType::SingleChoice | QuestionType::MultiChoice | QuestionType::TrueFalse) {
        options.clear();
    }
    let answer = normalize_answer(&i.qtype, &options, &i.answer)?;
    let q = Question {
        id: "x".into(),
        qtype: i.qtype.clone(),
        stem: i.stem.trim().to_string(),
        options: options.clone(),
        answer: answer.clone(),
        analysis: i.analysis.trim().to_string(),
        ai_analysis: None,
        score: None,
        subject: None,
        chapter: None,
        difficulty: None,
    };
    validate_question(&q)?;
    let chapter = match i.chapter.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(c) if c.chars().count() <= MAX_CHAPTER_CHARS && !c.chars().any(char::is_control) => Some(c.to_string()),
        _ => return Err(bad("invalid_chapter")),
    };
    Ok(Clean {
        qtype: i.qtype.to_string(),
        norm: stem_norm(&q.stem),
        stem: q.stem,
        options,
        answer,
        analysis: q.analysis,
        chapter,
        difficulty: i.difficulty.as_ref().map(|d| d.to_string()),
        tags: clean_tags(&i.tags)?,
    })
}

#[derive(Serialize, Debug, Clone)]
pub struct BankItem {
    id: String,
    subject_id: String,
    #[serde(rename = "type")]
    qtype: String,
    stem: String,
    options: Vec<String>,
    answer: String,
    analysis: String,
    chapter: Option<String>,
    difficulty: Option<String>,
    tags: Vec<String>,
    created_by: Option<String>,
    created_at: i64,
    updated_at: i64,
    archived_at: Option<i64>,
}

const COLS: &str = "id, subject_id, type, stem, options, answer, analysis, chapter, difficulty, tags, created_by, created_at, updated_at, archived_at";

fn row_item(r: &rusqlite::Row) -> rusqlite::Result<BankItem> {
    let json_list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
    Ok(BankItem {
        id: r.get(0)?,
        subject_id: r.get(1)?,
        qtype: r.get(2)?,
        stem: r.get(3)?,
        options: json_list(r.get(4)?),
        answer: r.get(5)?,
        analysis: r.get(6)?,
        chapter: r.get(7)?,
        difficulty: r.get(8)?,
        tags: json_list(r.get(9)?),
        created_by: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        archived_at: r.get(13)?,
    })
}

fn get_item(conn: &Connection, id: &str) -> Res<BankItem> {
    conn.query_row(&format!("SELECT {COLS} FROM question_bank_items WHERE id = ?1"), params![id], row_item)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn subject_exists(conn: &Connection, id: &str) -> Res<()> {
    let n: i64 = conn.query_row("SELECT count(*) FROM subjects WHERE id = ?1", params![id], |r| r.get(0)).map_err(db_err)?;
    if n == 0 {
        Err(err(StatusCode::NOT_FOUND, "not_found"))
    } else {
        Ok(())
    }
}

fn subject_count(conn: &Connection, subject_id: &str) -> Res<i64> {
    conn.query_row("SELECT count(*) FROM question_bank_items WHERE subject_id = ?1", params![subject_id], |r| r.get(0)).map_err(db_err)
}

/// Another active item in the subject with the same normalised stem (a warning, never a block).
fn find_duplicate(conn: &Connection, subject_id: &str, norm: &str, except: Option<&str>) -> Res<Option<String>> {
    conn.query_row(
        "SELECT id FROM question_bank_items WHERE subject_id = ?1 AND stem_norm = ?2 AND archived_at IS NULL AND (?3 IS NULL OR id != ?3) LIMIT 1",
        params![subject_id, norm, except],
        |r| r.get(0),
    )
    .optional()
    .map_err(db_err)
}

fn insert(conn: &Connection, admin: &User, subject_id: &str, c: &Clean, now: i64) -> Res<String> {
    let id = new_id();
    conn.execute(
        "INSERT INTO question_bank_items(id, subject_id, type, stem, stem_norm, options, answer, analysis, chapter, difficulty, tags, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            id, subject_id, c.qtype, c.stem, c.norm,
            serde_json::to_string(&c.options).unwrap_or_else(|_| "[]".into()),
            c.answer, c.analysis, c.chapter, c.difficulty,
            serde_json::to_string(&c.tags).unwrap_or_else(|_| "[]".into()),
            admin.id, now
        ],
    )
    .map_err(db_err)?;
    Ok(id)
}

// ───────── create / update ─────────

#[derive(Deserialize)]
pub struct CreateReq {
    subject_id: String,
    #[serde(flatten)]
    item: ItemIn,
}

#[derive(Serialize, Debug)]
pub struct Saved {
    item: BankItem,
    /// Id of another active item with the same stem, if any (a warning only).
    duplicate_of: Option<String>,
}

pub fn create_item(conn: &Connection, admin: &User, r: &CreateReq, now: i64) -> Res<Saved> {
    subject_exists(conn, &r.subject_id)?;
    let c = clean(&r.item)?;
    if subject_count(conn, &r.subject_id)? >= MAX_PER_SUBJECT {
        return Err(err(StatusCode::CONFLICT, "bank_full"));
    }
    let duplicate_of = find_duplicate(conn, &r.subject_id, &c.norm, None)?;
    let id = insert(conn, admin, &r.subject_id, &c, now)?;
    audit(conn, &admin.id, &id, "bank_create", &r.subject_id);
    Ok(Saved { item: get_item(conn, &id)?, duplicate_of })
}

pub fn update_item(conn: &Connection, admin: &User, id: &str, item: &ItemIn, now: i64) -> Res<Saved> {
    let existing = get_item(conn, id)?;
    let c = clean(item)?;
    let duplicate_of = if existing.archived_at.is_none() { find_duplicate(conn, &existing.subject_id, &c.norm, Some(id))? } else { None };
    conn.execute(
        "UPDATE question_bank_items SET type = ?2, stem = ?3, stem_norm = ?4, options = ?5, answer = ?6, analysis = ?7,
           chapter = ?8, difficulty = ?9, tags = ?10, updated_at = ?11 WHERE id = ?1",
        params![
            id, c.qtype, c.stem, c.norm,
            serde_json::to_string(&c.options).unwrap_or_else(|_| "[]".into()),
            c.answer, c.analysis, c.chapter, c.difficulty,
            serde_json::to_string(&c.tags).unwrap_or_else(|_| "[]".into()),
            now
        ],
    )
    .map_err(db_err)?;
    audit(conn, &admin.id, id, "bank_update", &existing.subject_id);
    Ok(Saved { item: get_item(conn, id)?, duplicate_of })
}

// ───────── list / facets ─────────

#[derive(Deserialize, Default)]
pub struct ListQuery {
    subject_id: Option<String>,
    #[serde(rename = "type")]
    qtype: Option<String>,
    difficulty: Option<String>,
    chapter: Option<String>,
    tag: Option<String>,
    q: Option<String>,
    /// `active` (default), `archived` or `all`.
    state: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct Page {
    items: Vec<BankItem>,
    total: i64,
}

fn like_pattern(raw: &str) -> String {
    format!("%{}%", stem_norm(raw).replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

pub fn list_items(conn: &Connection, f: &ListQuery) -> Res<Page> {
    let mut conds: Vec<String> = vec![];
    let mut args: Vec<Value> = vec![];
    // Adds `cond` (with one `?` placeholder) bound to `v`.
    fn add(conds: &mut Vec<String>, args: &mut Vec<Value>, cond: &str, v: String) {
        args.push(Value::Text(v));
        conds.push(cond.replace('?', &format!("?{}", args.len())));
    }
    if let Some(s) = f.subject_id.as_deref().filter(|s| !s.is_empty()) {
        add(&mut conds, &mut args, "subject_id = ?", s.to_string());
    }
    if let Some(t) = f.qtype.as_deref().filter(|s| !s.is_empty()) {
        add(&mut conds, &mut args, "type = ?", t.to_string());
    }
    if let Some(d) = f.difficulty.as_deref().filter(|s| !s.is_empty()) {
        // "none" selects items without a difficulty
        if d == "none" {
            conds.push("difficulty IS NULL".into());
        } else {
            add(&mut conds, &mut args, "difficulty = ?", d.to_string());
        }
    }
    if let Some(c) = f.chapter.as_deref().filter(|s| !s.is_empty()) {
        add(&mut conds, &mut args, "chapter = ?", c.to_string());
    }
    if let Some(t) = f.tag.as_deref().filter(|s| !s.is_empty()) {
        add(&mut conds, &mut args, "EXISTS (SELECT 1 FROM json_each(question_bank_items.tags) WHERE value = ?)", t.to_string());
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        add(&mut conds, &mut args, "stem_norm LIKE ? ESCAPE '\\'", like_pattern(q));
    }
    match f.state.as_deref().unwrap_or("active") {
        "active" => conds.push("archived_at IS NULL".into()),
        "archived" => conds.push("archived_at IS NOT NULL".into()),
        "all" => {}
        _ => return Err(bad("invalid_filter")),
    }
    let wh = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) FROM question_bank_items {wh}"), params_from_iter(args.iter()), |r| r.get(0))
        .map_err(db_err)?;
    let limit = f.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let offset = f.offset.unwrap_or(0).clamp(0, 10_000_000);
    let items = conn
        .prepare(&format!("SELECT {COLS} FROM question_bank_items {wh} ORDER BY created_at DESC, id LIMIT {limit} OFFSET {offset}"))
        .map_err(db_err)?
        .query_map(params_from_iter(args.iter()), row_item)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(Page { items, total })
}

#[derive(Serialize, Debug)]
pub struct Counted {
    value: String,
    count: i64,
}
#[derive(Serialize, Debug)]
pub struct Facets {
    active: i64,
    archived: i64,
    chapters: Vec<Counted>,
    tags: Vec<Counted>,
    types: Vec<Counted>,
    difficulties: Vec<Counted>,
}

#[derive(Deserialize)]
pub struct FacetQuery {
    subject_id: String,
}

fn counted(conn: &Connection, sql: &str, subject_id: &str) -> Res<Vec<Counted>> {
    conn.prepare(sql)
        .map_err(db_err)?
        .query_map(params![subject_id], |r| Ok(Counted { value: r.get(0)?, count: r.get(1)? }))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

/// Filter choices for a subject (active items only): chapters, tags, types, difficulties.
pub fn facets(conn: &Connection, subject_id: &str) -> Res<Facets> {
    let n = |cond: &str| -> Res<i64> {
        conn.query_row(&format!("SELECT count(*) FROM question_bank_items WHERE subject_id = ?1 AND {cond}"), params![subject_id], |r| r.get(0)).map_err(db_err)
    };
    Ok(Facets {
        active: n("archived_at IS NULL")?,
        archived: n("archived_at IS NOT NULL")?,
        chapters: counted(conn, "SELECT chapter, count(*) FROM question_bank_items WHERE subject_id = ?1 AND archived_at IS NULL AND chapter IS NOT NULL GROUP BY chapter ORDER BY chapter LIMIT 200", subject_id)?,
        tags: counted(conn, "SELECT j.value, count(*) FROM question_bank_items i, json_each(i.tags) j WHERE i.subject_id = ?1 AND i.archived_at IS NULL GROUP BY j.value ORDER BY count(*) DESC, j.value LIMIT 100", subject_id)?,
        types: counted(conn, "SELECT type, count(*) FROM question_bank_items WHERE subject_id = ?1 AND archived_at IS NULL GROUP BY type", subject_id)?,
        difficulties: counted(conn, "SELECT COALESCE(difficulty, 'none'), count(*) FROM question_bank_items WHERE subject_id = ?1 AND archived_at IS NULL GROUP BY 1", subject_id)?,
    })
}

// ───────── bulk actions & import ─────────

#[derive(Deserialize)]
pub struct BulkReq {
    ids: Vec<String>,
    /// `archive`, `restore` or `delete`.
    action: String,
}

pub fn bulk(conn: &Connection, admin: &User, r: &BulkReq, now: i64) -> Res<i64> {
    if r.ids.is_empty() || r.ids.len() > MAX_BULK {
        return Err(bad("invalid_selection"));
    }
    let (sql, action) = match r.action.as_str() {
        "archive" => ("UPDATE question_bank_items SET archived_at = ?2, updated_at = ?2 WHERE id = ?1 AND archived_at IS NULL", "bank_archive"),
        "restore" => ("UPDATE question_bank_items SET archived_at = NULL, updated_at = ?2 WHERE id = ?1 AND archived_at IS NOT NULL", "bank_restore"),
        "delete" => ("DELETE FROM question_bank_items WHERE id = ?1 AND ?2 > 0", "bank_delete"),
        _ => return Err(bad("invalid_action")),
    };
    let mut changed = 0;
    for id in &r.ids {
        changed += conn.execute(sql, params![id, now]).map_err(db_err)? as i64;
    }
    audit(conn, &admin.id, "bank", action, &format!("{changed} item(s)"));
    Ok(changed)
}

#[derive(Deserialize)]
pub struct ImportReq {
    subject_id: String,
    /// Raw items (from a browser bank, a CSV/XLSX parse or a generation result). Each is validated on its own.
    items: Vec<serde_json::Value>,
    /// Skip items whose stem already exists (in the bank or earlier in this batch).
    #[serde(default)]
    skip_duplicates: bool,
}

#[derive(Serialize, Debug)]
pub struct Rejected {
    index: usize,
    error: String,
}
#[derive(Serialize, Debug)]
pub struct ImportResult {
    created: i64,
    skipped_duplicates: i64,
    /// Items stored although another item with the same stem exists (only when `skip_duplicates` is off).
    duplicates_kept: i64,
    rejected: Vec<Rejected>,
}

fn error_code(e: &crate::relay::Err) -> String {
    serde_json::from_str::<serde_json::Value>(&e.1).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_else(|| "invalid_question".into())
}

pub fn import(conn: &Connection, admin: &User, r: &ImportReq, now: i64) -> Res<ImportResult> {
    subject_exists(conn, &r.subject_id)?;
    if r.items.is_empty() || r.items.len() > MAX_IMPORT {
        return Err(bad("invalid_import"));
    }
    let mut room = MAX_PER_SUBJECT - subject_count(conn, &r.subject_id)?;
    let mut out = ImportResult { created: 0, skipped_duplicates: 0, duplicates_kept: 0, rejected: vec![] };
    let mut seen: HashSet<String> = HashSet::new();
    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
    let result = (|| -> Res<()> {
        for (index, raw) in r.items.iter().enumerate() {
            let parsed = serde_json::from_value::<ItemIn>(raw.clone()).map_err(|_| bad("invalid_question")).and_then(|i| clean(&i));
            let c = match parsed {
                Ok(c) => c,
                Err(e) => {
                    out.rejected.push(Rejected { index, error: error_code(&e) });
                    continue;
                }
            };
            let dup = seen.contains(&c.norm) || find_duplicate(conn, &r.subject_id, &c.norm, None)?.is_some();
            if dup && r.skip_duplicates {
                out.skipped_duplicates += 1;
                continue;
            }
            if room <= 0 {
                out.rejected.push(Rejected { index, error: "bank_full".into() });
                continue;
            }
            insert(conn, admin, &r.subject_id, &c, now)?;
            room -= 1;
            seen.insert(c.norm);
            out.created += 1;
            if dup {
                out.duplicates_kept += 1;
            }
        }
        audit(conn, &admin.id, &r.subject_id, "bank_import", &format!("created={} skipped={} rejected={}", out.created, out.skipped_duplicates, out.rejected.len()));
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(db_err)?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(out)
}

// ───────── handlers ─────────

/// 500 items of up to ~15 KB each fit well under this; the default (2 MB) is too small for a big bank.
pub fn import_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(8 * 1024 * 1024)
}

pub async fn list_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(f): Query<ListQuery>) -> Res<Json<Page>> {
    require_admin(&s, &h)?;
    list_items(&*lock(&s)?, &f).map(Json)
}

pub async fn facets_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(f): Query<FacetQuery>) -> Res<Json<Facets>> {
    require_admin(&s, &h)?;
    facets(&*lock(&s)?, &f.subject_id).map(Json)
}

pub async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<CreateReq>) -> Res<(StatusCode, Json<Saved>)> {
    let admin = require_admin(&s, &h)?;
    create_item(&*lock(&s)?, &admin, &r, now_ms()).map(|v| (StatusCode::CREATED, Json(v)))
}

pub async fn update_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<ItemIn>) -> Res<Json<Saved>> {
    let admin = require_admin(&s, &h)?;
    update_item(&*lock(&s)?, &admin, &id, &r, now_ms()).map(Json)
}

pub async fn bulk_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<BulkReq>) -> Res<Json<serde_json::Value>> {
    let admin = require_admin(&s, &h)?;
    let changed = bulk(&*lock(&s)?, &admin, &r, now_ms())?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

pub async fn import_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<ImportReq>) -> Res<Json<ImportResult>> {
    let admin = require_admin(&s, &h)?;
    import(&*lock(&s)?, &admin, &r, now_ms()).map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use serde_json::json;

    struct W {
        conn: Connection,
        admin: User,
        subject: String,
    }
    fn world() -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s2','i1','رياضيات',0)", []).unwrap();
        W { conn, admin, subject: "s1".into() }
    }
    fn item(v: serde_json::Value) -> ItemIn {
        serde_json::from_value(v).unwrap()
    }
    fn mc(stem: &str, answer: &str) -> serde_json::Value {
        json!({"type":"single_choice","stem":stem,"options":["أ","ب","ج"],"answer":answer})
    }

    #[test]
    fn answers_are_normalised_and_garbage_rejected() {
        let o = |n: usize| (0..n).map(|i| format!("o{i}")).collect::<Vec<_>>();
        let sc = QuestionType::SingleChoice;
        let mu = QuestionType::MultiChoice;
        assert_eq!(normalize_answer(&sc, &o(4), "b").unwrap(), "B");
        assert_eq!(normalize_answer(&sc, &o(4), "B. القاهرة").unwrap(), "B");
        assert_eq!(normalize_answer(&mu, &o(4), "C, a").unwrap(), "AC");
        assert_eq!(normalize_answer(&mu, &o(4), "CA،A").unwrap(), "AC");
        assert!(normalize_answer(&sc, &o(3), "D").is_err(), "letter beyond the options");
        assert!(normalize_answer(&sc, &o(4), "AB").is_err(), "single choice has exactly one letter");
        assert!(normalize_answer(&sc, &o(4), "القاهرة").is_err(), "text instead of a letter");
        assert!(normalize_answer(&mu, &o(4), "").is_err());
        let tf = QuestionType::TrueFalse;
        for t in ["True", "صحيح", "صَحِيح", "A", "نعم", "对"] {
            assert_eq!(normalize_answer(&tf, &[], t).unwrap(), "A", "{t}");
        }
        for f in ["false", "خطأ", "خطا", "B", "لا", "错误"] {
            assert_eq!(normalize_answer(&tf, &[], f).unwrap(), "B", "{f}");
        }
        assert!(normalize_answer(&tf, &[], "ربما").is_err());
        assert_eq!(normalize_answer(&QuestionType::FillBlank, &[], "  القاهرة ").unwrap(), "القاهرة");
    }

    #[test]
    fn create_validates_and_defaults_true_false_options() {
        let w = world();
        let mk = |v| create_item(&w.conn, &w.admin, &CreateReq { subject_id: w.subject.clone(), item: item(v) }, 1000);
        let tf = mk(json!({"type":"true_false","stem":"الأرض كروية","answer":"صحيح","tags":["جغرافيا"," جغرافيا ","علوم"],"difficulty":"easy","chapter":"  الوحدة 1 "})).unwrap();
        assert_eq!((tf.item.options.clone(), tf.item.answer.as_str(), tf.item.tags.clone(), tf.item.chapter.as_deref()), (vec!["صحيح".to_string(), "خطأ".to_string()], "A", vec!["جغرافيا".to_string(), "علوم".to_string()], Some("الوحدة 1")));
        let fb = mk(json!({"type":"fill_blank","stem":"عاصمة مصر ___","options":["x","y"],"answer":"القاهرة"})).unwrap();
        assert!(fb.item.options.is_empty(), "options are dropped for types that do not use them");
        for (label, v) in [
            ("empty stem", json!({"type":"short_answer","stem":"  ","answer":"x"})),
            ("one option", json!({"type":"single_choice","stem":"س","options":["a"],"answer":"A"})),
            ("bad type", json!({"type":"essay","stem":"س","answer":"x"})),
            ("too many tags", json!({"type":"short_answer","stem":"س","answer":"x","tags":["1","2","3","4","5","6","7","8","9","10","11"]})),
            ("long tag", json!({"type":"short_answer","stem":"س","answer":"x","tags":["x".repeat(31)]})),
            ("bad answer", json!({"type":"single_choice","stem":"س","options":["a","b"],"answer":"Z"})),
        ] {
            let parsed = serde_json::from_value::<ItemIn>(v).map_err(|_| ()).and_then(|i| create_item(&w.conn, &w.admin, &CreateReq { subject_id: w.subject.clone(), item: i }, 1).map_err(|_| ()));
            assert!(parsed.is_err(), "{label} must be rejected");
        }
        assert_eq!(create_item(&w.conn, &w.admin, &CreateReq { subject_id: "nope".into(), item: item(mc("س", "A")) }, 1).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(subject_count(&w.conn, "s1").unwrap(), 2, "rejected items leave nothing behind");
    }

    #[test]
    fn duplicates_warn_but_never_block_and_ignore_arabic_variants_and_archived() {
        let w = world();
        let c = |stem: &str| create_item(&w.conn, &w.admin, &CreateReq { subject_id: w.subject.clone(), item: item(mc(stem, "A")) }, 5).unwrap();
        let first = c("ما هي أكبر دولة؟");
        assert!(first.duplicate_of.is_none());
        let second = c("  ما هي   اكبر دولة؟ ");
        assert_eq!(second.duplicate_of.as_deref(), Some(first.item.id.as_str()), "hamza and spacing differences still match");
        // another subject is independent
        let other = create_item(&w.conn, &w.admin, &CreateReq { subject_id: "s2".into(), item: item(mc("ما هي أكبر دولة؟", "A")) }, 5).unwrap();
        assert!(other.duplicate_of.is_none());
        // archived items do not count
        bulk(&w.conn, &w.admin, &BulkReq { ids: vec![first.item.id.clone(), second.item.id.clone()], action: "archive".into() }, 9).unwrap();
        assert!(c("ما هي أكبر دولة؟").duplicate_of.is_none());
        // editing an item onto an existing stem warns too, but not about itself
        let a = c("سؤال فريد أ");
        let b = c("سؤال فريد ب");
        assert!(update_item(&w.conn, &w.admin, &a.item.id, &item(mc("سؤال فريد أ", "B")), 20).unwrap().duplicate_of.is_none());
        assert_eq!(update_item(&w.conn, &w.admin, &a.item.id, &item(mc("سؤال فريد ب", "B")), 21).unwrap().duplicate_of.as_deref(), Some(b.item.id.as_str()));
    }

    #[test]
    fn list_filters_search_paging_and_states() {
        let w = world();
        let mk = |v: serde_json::Value| create_item(&w.conn, &w.admin, &CreateReq { subject_id: w.subject.clone(), item: item(v) }, 100).unwrap().item;
        let a = mk(json!({"type":"single_choice","stem":"أُسس البرمجة الكائنية","options":["a","b"],"answer":"A","difficulty":"easy","chapter":"1","tags":["oop","أساسيات"]}));
        let b = mk(json!({"type":"short_answer","stem":"اشرح الوراثة","answer":"x","difficulty":"hard","chapter":"2","tags":["oop"]}));
        let c = mk(json!({"type":"true_false","stem":"100% من الشيفرة تُختبر؟","answer":"لا"}));
        let q = |f: ListQuery| list_items(&w.conn, &f).unwrap();
        let base = || ListQuery { subject_id: Some("s1".into()), ..Default::default() };
        assert_eq!(q(base()).total, 3);
        assert_eq!(q(ListQuery { qtype: Some("short_answer".into()), ..base() }).items[0].id, b.id);
        assert_eq!(q(ListQuery { difficulty: Some("easy".into()), ..base() }).total, 1);
        assert_eq!(q(ListQuery { difficulty: Some("none".into()), ..base() }).items[0].id, c.id, "items without a difficulty");
        assert_eq!(q(ListQuery { chapter: Some("2".into()), ..base() }).total, 1);
        assert_eq!(q(ListQuery { tag: Some("oop".into()), ..base() }).total, 2);
        assert_eq!(q(ListQuery { tag: Some("oo".into()), ..base() }).total, 0, "tags match exactly");
        assert_eq!(q(ListQuery { q: Some("اسس البرمجه".into()), ..base() }).total, 0, "ة and ه are different letters, so this does not match");
        assert_eq!(q(ListQuery { q: Some("اسس".into()), ..base() }).items[0].id, a.id, "diacritics/hamza-insensitive search");
        assert_eq!(q(ListQuery { q: Some("100%".into()), ..base() }).items[0].id, c.id, "% is literal");
        assert_eq!(q(ListQuery { q: Some("%".into()), ..base() }).total, 1, "wildcards are escaped, not matching everything");
        assert_eq!(q(ListQuery { subject_id: Some("s2".into()), ..Default::default() }).total, 0);
        let p1 = q(ListQuery { limit: Some(2), ..base() });
        let p2 = q(ListQuery { limit: Some(2), offset: Some(2), ..base() });
        assert_eq!((p1.items.len(), p1.total, p2.items.len()), (2, 3, 1));
        assert!(p1.items.iter().all(|i| i.id != p2.items[0].id));
        bulk(&w.conn, &w.admin, &BulkReq { ids: vec![a.id.clone()], action: "archive".into() }, 7).unwrap();
        assert_eq!((q(base()).total, q(ListQuery { state: Some("archived".into()), ..base() }).total, q(ListQuery { state: Some("all".into()), ..base() }).total), (2, 1, 3));
        assert!(list_items(&w.conn, &ListQuery { state: Some("zzz".into()), ..base() }).is_err());
        let f = facets(&w.conn, "s1").unwrap();
        assert_eq!((f.active, f.archived, f.chapters.len()), (2, 1, 1), "facets count active items only");
        assert_eq!(f.tags.iter().map(|t| (t.value.as_str(), t.count)).collect::<Vec<_>>(), vec![("oop", 1)]);
    }

    #[test]
    fn bulk_archive_restore_delete_and_limits() {
        let w = world();
        let ids: Vec<String> = (0..3).map(|i| create_item(&w.conn, &w.admin, &CreateReq { subject_id: "s1".into(), item: item(mc(&format!("س{i}"), "A")) }, 1).unwrap().item.id).collect();
        let run = |action: &str, ids: Vec<String>| bulk(&w.conn, &w.admin, &BulkReq { ids, action: action.into() }, 50);
        assert_eq!(run("archive", ids.clone()).unwrap(), 3);
        assert_eq!(run("archive", ids.clone()).unwrap(), 0, "already archived: nothing changes");
        assert_eq!(run("restore", ids[..2].to_vec()).unwrap(), 2);
        assert_eq!(run("delete", vec![ids[0].clone(), "ghost".into()]).unwrap(), 1);
        assert!(run("explode", ids.clone()).is_err() && run("delete", vec![]).is_err());
        assert!(run("delete", (0..501).map(|i| i.to_string()).collect()).is_err());
        assert_eq!(subject_count(&w.conn, "s1").unwrap(), 2);
        let log: String = w.conn.query_row("SELECT group_concat(action, ',') FROM audit_log", [], |r| r.get(0)).unwrap();
        assert!(log.contains("bank_archive") && log.contains("bank_restore") && log.contains("bank_delete"));
    }

    #[test]
    fn import_reports_per_item_skips_duplicates_and_is_all_or_nothing_on_failure() {
        let w = world();
        let existing = create_item(&w.conn, &w.admin, &CreateReq { subject_id: "s1".into(), item: item(mc("سؤال موجود", "A")) }, 1).unwrap();
        let items = vec![
            mc("سؤال جديد 1", "B"),
            mc("سؤال موجود", "A"),                                       // already in the bank
            mc("سؤال جديد 1", "C"),                                      // repeated inside the batch
            json!({"type":"single_choice","stem":"","options":["a","b"],"answer":"A"}), // invalid
            json!({"nonsense": true}),                                   // not even a question
            // a local-bank question carries extra fields that must be ignored
            json!({"id":"loc-1","type":"fill_blank","stem":"عاصمة اليمن ___","answer":"صنعاء","analysis":"","aiAnalysis":"x","score":2,"subject":"جغرافيا","chapter":"الوحدة 1","difficulty":"medium"}),
        ];
        let r = import(&w.conn, &w.admin, &ImportReq { subject_id: "s1".into(), items: items.clone(), skip_duplicates: true }, 10).unwrap();
        assert_eq!((r.created, r.skipped_duplicates, r.duplicates_kept, r.rejected.len()), (2, 2, 0, 2));
        assert_eq!(r.rejected.iter().map(|x| x.index).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(r.rejected[0].error, "invalid_question");
        assert_eq!(subject_count(&w.conn, "s1").unwrap(), 3);
        let r2 = import(&w.conn, &w.admin, &ImportReq { subject_id: "s1".into(), items: items.clone(), skip_duplicates: false }, 11).unwrap();
        assert_eq!((r2.created, r2.duplicates_kept, r2.skipped_duplicates), (4, 4, 0), "without skipping, duplicates are kept and counted");
        assert!(import(&w.conn, &w.admin, &ImportReq { subject_id: "s1".into(), items: vec![], skip_duplicates: true }, 12).is_err());
        assert!(import(&w.conn, &w.admin, &ImportReq { subject_id: "s1".into(), items: vec![json!({}); 501], skip_duplicates: true }, 12).is_err());
        assert_eq!(import(&w.conn, &w.admin, &ImportReq { subject_id: "nope".into(), items: items.clone(), skip_duplicates: true }, 12).unwrap_err().0, StatusCode::NOT_FOUND);
        let loc = list_items(&w.conn, &ListQuery { q: Some("عاصمة اليمن".into()), ..Default::default() }).unwrap().items.remove(0);
        assert_eq!((loc.chapter.as_deref(), loc.difficulty.as_deref(), loc.answer.as_str()), (Some("الوحدة 1"), Some("medium"), "صنعاء"));
        let _ = existing;
    }

    #[test]
    fn deleting_the_subject_or_the_creator_is_handled_by_the_schema() {
        let w = world();
        let i = create_item(&w.conn, &w.admin, &CreateReq { subject_id: "s1".into(), item: item(mc("س", "A")) }, 1).unwrap().item;
        w.conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        w.conn.execute("DELETE FROM users WHERE id = ?1", params![w.admin.id]).unwrap();
        assert_eq!(get_item(&w.conn, &i.id).unwrap().created_by, None, "the item outlives its creator");
        w.conn.execute("DELETE FROM subjects WHERE id = 's1'", []).unwrap();
        assert!(get_item(&w.conn, &i.id).is_err(), "items follow their subject");
    }
}
