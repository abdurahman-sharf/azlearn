//! Teaching assignments (teacher <-> subject, admin-approved), student placement and
//! enrollment, following teachers, public teacher/subject pages.

use crate::platform::{audit, authenticate, bad, db_err, like_pattern, lock, opt_text, require_active, require_admin, require_role, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS teacher_subjects (
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected')),
  created_at INTEGER NOT NULL,
  decided_at INTEGER,
  PRIMARY KEY (teacher_id, subject_id)
);
CREATE INDEX IF NOT EXISTS idx_ts_subject ON teacher_subjects(subject_id, status);
CREATE TABLE IF NOT EXISTS student_placement (
  student_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  unit_id TEXT REFERENCES org_units(id) ON DELETE SET NULL
);
CREATE TABLE IF NOT EXISTS subject_enrollments (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, subject_id)
);
CREATE INDEX IF NOT EXISTS idx_enroll_subject ON subject_enrollments(subject_id);
CREATE TABLE IF NOT EXISTS teacher_follows (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, teacher_id)
);
CREATE INDEX IF NOT EXISTS idx_follow_teacher ON teacher_follows(teacher_id);
";

// ───────── profile ─────────

#[derive(Deserialize, Default)]
pub struct ProfilePatch {
    full_name: Option<String>,
    bio: Option<String>,
    /// `school`, `institute` or `university`; anything else is refused. There is no way to clear it here.
    institution_type: Option<String>,
}

/// The kinds of institution a person can say they belong to (same list as `institutions.type`).
pub(crate) fn valid_institution_type(raw: &str) -> Res<&str> {
    match raw {
        t @ ("school" | "institute" | "university") => Ok(t),
        _ => Err(bad("invalid_type")),
    }
}

/// Edits the caller's own profile row (`user.id` comes from the session). Every field is validated before the
/// first write, so a bad value never leaves a half-applied change.
fn update_profile(conn: &Connection, user: &User, p: &ProfilePatch) -> Res<()> {
    let name = match &p.full_name {
        Some(n) => {
            let t = n.trim();
            if t.is_empty() || t.chars().count() > 120 {
                return Err(bad("invalid_name"));
            }
            Some(t.to_string())
        }
        None => None,
    };
    let bio = if p.bio.is_some() { Some(opt_text(&p.bio, 1000, "bio_too_long")?) } else { None };
    let inst = match &p.institution_type {
        Some(t) => Some(valid_institution_type(t)?),
        None => None,
    };
    if let Some(n) = name {
        conn.execute("UPDATE users SET full_name = ?1 WHERE id = ?2", params![n, user.id]).map_err(db_err)?;
    }
    if let Some(b) = bio {
        conn.execute("UPDATE users SET bio = ?1 WHERE id = ?2", params![b, user.id]).map_err(db_err)?;
    }
    if let Some(t) = inst {
        conn.execute("UPDATE users SET institution_type = ?1 WHERE id = ?2", params![t, user.id]).map_err(db_err)?;
    }
    Ok(())
}

pub async fn update_profile_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(p): Json<ProfilePatch>,
) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    update_profile(&*lock(&state)?, &user, &p)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── teaching assignments ─────────

/// An assignment as the PUBLIC pages show it (the teacher page lists the approved ones). It carries no dates, no
/// admin note and no activity flags: those belong to the teacher and the admin only ([`TeachingExtra`]).
#[derive(Serialize, Debug)]
pub struct Teaching {
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    institution_id: String,
    institution_name: String,
    status: String,
}

/// What only the assignment's teacher and the admins may see: when it was asked for / decided, the admin's note on
/// the decision, where the subject sits in the institution, and whether the subject and its institution are on.
/// Deliberately a separate struct (flattened next to [`Teaching`] in the two private DTOs) so it can never leak
/// into the public teacher page.
#[derive(Serialize, Debug)]
pub struct TeachingExtra {
    created_at: i64,
    decided_at: Option<i64>,
    /// The admin's optional note on the decision (rejection reason, why it was withdrawn).
    reason: Option<String>,
    /// Unit names from the root down to the subject's own unit; empty when the subject hangs on the institution.
    unit_path: Vec<String>,
    subject_active: bool,
    institution_active: bool,
}

/// A row of `GET /teaching`: the teacher's own assignments.
#[derive(Serialize, Debug)]
pub struct MyTeaching {
    #[serde(flatten)]
    base: Teaching,
    #[serde(flatten)]
    extra: TeachingExtra,
}

/// An assignment row as the admin sees it: the standard fields plus the state of the teacher's *account*, so a
/// request from a teacher who is still waiting for approval is recognisable.
#[derive(Serialize, Debug)]
pub struct AdminTeaching {
    #[serde(flatten)]
    base: Teaching,
    #[serde(flatten)]
    extra: TeachingExtra,
    teacher_status: String,
}

const TEACHING_COLS: &str = "ts.teacher_id, u.full_name, ts.subject_id, s.name_ar, i.id, i.name_ar, ts.status";
const TEACHING_FROM: &str = "FROM teacher_subjects ts
     JOIN users u ON u.id = ts.teacher_id
     JOIN subjects s ON s.id = ts.subject_id
     JOIN institutions i ON i.id = s.institution_id";
/// Columns 7..=12 of the private DTOs, read by [`read_detail`].
const DETAIL_COLS: &str = ", ts.created_at, ts.decided_at, ts.reason, s.unit_id, s.is_active, i.is_active";

/// The shared assignment query; `extra` adds columns after the standard seven (e.g. [`DETAIL_COLS`]).
fn teaching_select(extra: &str) -> String {
    format!("SELECT {TEACHING_COLS}{extra} {TEACHING_FROM}")
}

fn map_teaching(r: &rusqlite::Row) -> rusqlite::Result<Teaching> {
    Ok(Teaching {
        teacher_id: r.get(0)?,
        teacher_name: r.get(1)?,
        subject_id: r.get(2)?,
        subject_name: r.get(3)?,
        institution_id: r.get(4)?,
        institution_name: r.get(5)?,
        status: r.get(6)?,
    })
}

/// A row read from [`DETAIL_COLS`] whose unit path is still to be looked up.
struct RawDetail {
    base: Teaching,
    created_at: i64,
    decided_at: Option<i64>,
    reason: Option<String>,
    unit_id: Option<String>,
    subject_active: bool,
    institution_active: bool,
}

fn read_detail(r: &rusqlite::Row) -> rusqlite::Result<RawDetail> {
    Ok(RawDetail {
        base: map_teaching(r)?,
        created_at: r.get(7)?,
        decided_at: r.get(8)?,
        reason: r.get(9)?,
        unit_id: r.get(10)?,
        subject_active: r.get(11)?,
        institution_active: r.get(12)?,
    })
}

/// Names of the unit chain above (and including) `unit_id`, root first (e.g. department > level > term).
pub(crate) fn unit_path(conn: &Connection, unit_id: Option<String>) -> Res<Vec<String>> {
    let mut path = Vec::new();
    let mut cur = unit_id;
    while let Some(uid) = cur {
        let row: Option<(String, Option<String>)> = conn
            .query_row("SELECT name_ar, parent_id FROM org_units WHERE id = ?1", params![uid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(db_err)?;
        let Some((name, parent)) = row else { break };
        path.push(name);
        cur = parent;
        if path.len() > 8 {
            break;
        }
    }
    path.reverse();
    Ok(path)
}

/// Splits the raw rows into the base DTO and its [`TeachingExtra`], looking each distinct unit's path up once.
fn finish_details(conn: &Connection, raws: Vec<RawDetail>) -> Res<Vec<(Teaching, TeachingExtra)>> {
    let mut paths: std::collections::HashMap<Option<String>, Vec<String>> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(raws.len());
    for r in raws {
        if !paths.contains_key(&r.unit_id) {
            let p = unit_path(conn, r.unit_id.clone())?;
            paths.insert(r.unit_id.clone(), p);
        }
        out.push((
            r.base,
            TeachingExtra {
                created_at: r.created_at,
                decided_at: r.decided_at,
                reason: r.reason,
                unit_path: paths[&r.unit_id].clone(),
                subject_active: r.subject_active,
                institution_active: r.institution_active,
            },
        ));
    }
    Ok(out)
}

fn my_teaching(conn: &Connection, teacher_id: &str) -> Res<Vec<MyTeaching>> {
    let raws = conn
        .prepare(&format!("{} WHERE ts.teacher_id = ?1 ORDER BY i.name_ar, s.name_ar", teaching_select(DETAIL_COLS)))
        .map_err(db_err)?
        .query_map(params![teacher_id], read_detail)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(finish_details(conn, raws)?.into_iter().map(|(base, extra)| MyTeaching { base, extra }).collect())
}

pub async fn my_teaching_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<MyTeaching>>> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    my_teaching(&*lock(&state)?, &user.id).map(Json)
}

#[derive(Deserialize)]
pub struct SubjectRef {
    subject_id: String,
}

/// How many requests a teacher may have waiting for the admin at once. Accounts are open to self-registration and a
/// teacher account may ask for subjects while it is itself still unapproved, so without a ceiling one throw-away
/// account could queue a request for every subject and bury the genuine ones.
pub(crate) const MAX_OPEN_TEACHING_REQUESTS: i64 = 20;

/// A teacher asks to teach a subject; it stays `pending` until an admin approves. Asking again after a rejection
/// starts a fresh request: the old decision's date and note are cleared with it.
fn request_teaching(conn: &Connection, teacher: &User, subject_id: &str) -> Res<()> {
    let active: Option<bool> = conn
        .query_row(
            "SELECT s.is_active AND i.is_active FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if active != Some(true) {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT status FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2",
            params![teacher.id, subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if matches!(existing.as_deref(), Some("pending") | Some("approved")) {
        return Err(err(StatusCode::CONFLICT, "already_requested"));
    }
    let open: i64 = conn
        .query_row("SELECT count(*) FROM teacher_subjects WHERE teacher_id = ?1 AND status = 'pending'", params![teacher.id], |r| r.get(0))
        .map_err(db_err)?;
    if open >= MAX_OPEN_TEACHING_REQUESTS {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too_many_pending_requests"));
    }
    match existing.as_deref() {
        Some(_) => {
            conn.execute(
                "UPDATE teacher_subjects SET status = 'pending', created_at = ?3, decided_at = NULL, reason = NULL WHERE teacher_id = ?1 AND subject_id = ?2",
                params![teacher.id, subject_id, now_ms()],
            )
            .map_err(db_err)?;
        }
        None => {
            conn.execute(
                "INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1, ?2, 'pending', ?3)",
                params![teacher.id, subject_id, now_ms()],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}

pub async fn request_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SubjectRef>,
) -> Res<StatusCode> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    request_teaching(&*lock(&state)?, &user, &req.subject_id)?;
    Ok(StatusCode::CREATED)
}

/// Who may use the "ask to teach a subject" flow: a teacher whose account is active **or still waiting for approval**
/// (so the request can already be in the admin's queue when the account is approved). Rejected and suspended teachers
/// — and everyone who is not a teacher — are refused. The flow is exactly five routes, so a pending teacher can
/// actually complete it: `POST /teaching` (ask), `GET /teaching` (see the requests), `GET /teaching/{id}/impact` (what
/// withdrawing would hide), `DELETE /teaching/{id}` (withdraw one) and — to find a subject id at all — the catalogue
/// reads, see [`may_browse_catalog`]. That is all a pending account gets: the requested content stays invisible until
/// the account is active *and* the assignment approved (`visible()`), and every other teacher route still goes
/// through `require_role`, which demands an active account.
pub(crate) fn may_request_teaching(user: User) -> Res<User> {
    if user.role == "teacher" && (user.status == "active" || user.status == "pending") {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Who may read the institution list and an institution's structure (`GET /institutions[/{id}/structure]`): any
/// active account, plus a teacher still waiting for approval — they need the catalogue to pick the subjects they
/// ask for ([`may_request_teaching`]). Pending students and rejected/suspended accounts of any role stay out.
pub(crate) fn may_browse_catalog(user: &User) -> bool {
    user.status == "active" || (user.role == "teacher" && user.status == "pending")
}

/// The teacher's own withdrawal of a request or of an approved subject (idempotent: nothing to withdraw is fine).
/// The content they published there is kept, it just stops being shown. Audited with what the subject was and how
/// many items it took out of sight.
fn withdraw_teaching(conn: &Connection, user: &User, subject_id: &str) -> Res<()> {
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let previous: Option<String> = tx
        .query_row("SELECT status FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2", params![user.id, subject_id], |r| r.get(0))
        .optional()
        .map_err(db_err)?;
    let Some(previous) = previous else { return Ok(()) };
    let hidden = if previous == "approved" { teaching_impact(&tx, &user.id, subject_id, now)?.total() } else { 0 };
    tx.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2", params![user.id, subject_id]).map_err(db_err)?;
    audit(&tx, &user.id, &user.id, "teaching_withdrawn", &format!("{subject_id} was {previous}, hidden={hidden}"));
    tx.commit().map_err(db_err)
}

pub async fn drop_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<StatusCode> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    withdraw_teaching(&*lock(&state)?, &user, &subject_id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── what losing a subject would hide ─────────

/// How many of a teacher's items on one subject students can see right now (and would stop seeing if the assignment
/// went away): published posts and courses, scheduled live sessions that have not ended, published exams. Drafts,
/// cancelled and finished sessions, closed/archived exams and anything already hidden (for whatever reason) are not
/// counted: they would not change.
#[derive(Serialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Impact {
    posts: i64,
    courses: i64,
    live: i64,
    exams: i64,
}

impl Impact {
    pub fn total(&self) -> i64 {
        self.posts + self.courses + self.live + self.exams
    }
}

/// The ONE counting helper behind the teacher's and the admin's impact endpoints, the withdrawal audit row and the
/// "revoked, N items hidden" notification. It counts with the same visibility predicates students are served with
/// ([`crate::platform_content::visible`], [`crate::platform_content::exam_standing`]), so what it reports is exactly
/// what would disappear.
pub(crate) fn teaching_impact(conn: &Connection, teacher_id: &str, subject_id: &str, now: i64) -> Res<Impact> {
    use crate::platform_content::{exam_standing, visible};
    let count = |sql: String, with_now: bool| -> Res<i64> {
        if with_now {
            conn.query_row(&sql, params![teacher_id, subject_id, now], |r| r.get(0)).map_err(db_err)
        } else {
            conn.query_row(&sql, params![teacher_id, subject_id], |r| r.get(0)).map_err(db_err)
        }
    };
    let content = |table: &str, a: &str, status: &str, extra: &str| {
        format!(
            "SELECT count(*) FROM {table} {a} JOIN users u ON u.id = {a}.teacher_id JOIN subjects s ON s.id = {a}.subject_id
             WHERE {a}.teacher_id = ?1 AND {a}.subject_id = ?2 AND {}{extra}",
            visible(a, status)
        )
    };
    Ok(Impact {
        posts: count(content("posts", "p", "published", ""), false)?,
        courses: count(content("courses", "c", "published", ""), false)?,
        live: count(content("live_sessions", "l", "scheduled", " AND l.starts_at + l.duration_min * 60000 > ?3"), true)?,
        exams: count(
            format!(
                "SELECT count(*) FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
                 WHERE a.teacher_id = ?1 AND a.subject_id = ?2 AND a.status = 'published' AND {}",
                exam_standing("a")
            ),
            false,
        )?,
    })
}

/// 404 unless the teacher has an assignment row (any status) for the subject.
fn require_assignment_row(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<()> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2)",
            params![teacher_id, subject_id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if exists {
        Ok(())
    } else {
        Err(err(StatusCode::NOT_FOUND, "not_found"))
    }
}

/// The impact for one of the CALLER's own assignments (`teacher_id` is the session's, never a parameter).
fn my_impact(conn: &Connection, teacher: &User, subject_id: &str, now: i64) -> Res<Impact> {
    require_assignment_row(conn, &teacher.id, subject_id)?;
    teaching_impact(conn, &teacher.id, subject_id, now)
}

pub async fn teaching_impact_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<Json<Impact>> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    my_impact(&*lock(&state)?, &user, &subject_id, now_ms()).map(Json)
}

#[derive(Deserialize)]
pub struct ImpactQuery {
    teacher_id: String,
    subject_id: String,
}

pub async fn admin_teaching_impact_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ImpactQuery>,
) -> Res<Json<Impact>> {
    require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    require_assignment_row(&conn, &q.teacher_id, &q.subject_id)?;
    teaching_impact(&conn, &q.teacher_id, &q.subject_id, now_ms()).map(Json)
}

// ───────── the admin's queue ─────────

const ADMIN_DEFAULT_LIMIT: i64 = 50;
const ADMIN_MAX_LIMIT: i64 = 200;
const ADMIN_MAX_SEARCH_CHARS: usize = 100;

/// `GET /admin/teaching` query. Blank values count as absent.
#[derive(Deserialize, Default, Debug)]
pub struct AdminTeachingQuery {
    /// `pending` | `approved` | `rejected`.
    status: Option<String>,
    /// Matches the teacher's name or e-mail, or the subject's name (either language).
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct AdminTeachingList {
    items: Vec<AdminTeaching>,
    /// Rows matching the filters, before `limit`/`offset`.
    total: i64,
}

pub async fn admin_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<AdminTeachingQuery>,
) -> Res<Json<AdminTeachingList>> {
    require_admin(&state, &headers)?;
    admin_teaching(&*lock(&state)?, &q).map(Json)
}

/// One page of the assignments for the admin, plus the total. Requests from active accounts come first: those from
/// accounts still awaiting approval can wait for the account decision and must not push real requests off the page.
/// Within that, the oldest request comes first (so nothing waits forever behind newer ones).
fn admin_teaching(conn: &Connection, q: &AdminTeachingQuery) -> Res<AdminTeachingList> {
    let blank = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let status = match blank(&q.status).as_deref() {
        None => None,
        Some(s @ ("pending" | "approved" | "rejected")) => Some(s.to_string()),
        Some(_) => return Err(bad("invalid_status")),
    };
    let search = blank(&q.q);
    if search.as_deref().map_or(false, |s| s.chars().count() > ADMIN_MAX_SEARCH_CHARS) {
        return Err(bad("invalid_query"));
    }
    let like = search.as_deref().map(like_pattern);
    let (limit, offset) = (q.limit.unwrap_or(ADMIN_DEFAULT_LIMIT).clamp(1, ADMIN_MAX_LIMIT), q.offset.unwrap_or(0).max(0));
    let filter = "(?1 IS NULL OR ts.status = ?1)
        AND (?2 IS NULL OR lower(u.full_name) LIKE ?2 ESCAPE '\\' OR lower(u.email) LIKE ?2 ESCAPE '\\'
             OR lower(s.name_ar) LIKE ?2 ESCAPE '\\' OR lower(COALESCE(s.name_en, '')) LIKE ?2 ESCAPE '\\')";
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) {TEACHING_FROM} WHERE {filter}"), params![status, like], |r| r.get(0))
        .map_err(db_err)?;
    let sql = format!(
        "{} WHERE {filter} ORDER BY (u.status = 'active') DESC, ts.created_at ASC, ts.teacher_id, ts.subject_id LIMIT ?3 OFFSET ?4",
        teaching_select(&format!("{DETAIL_COLS}, u.status"))
    );
    let raws = conn
        .prepare(&sql)
        .map_err(db_err)?
        .query_map(params![status, like, limit, offset], |r| Ok((read_detail(r)?, r.get::<_, String>(13)?)))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let (details, account): (Vec<RawDetail>, Vec<String>) = raws.into_iter().unzip();
    let items = finish_details(conn, details)?
        .into_iter()
        .zip(account)
        .map(|((base, extra), teacher_status)| AdminTeaching { base, extra, teacher_status })
        .collect();
    Ok(AdminTeachingList { items, total })
}

// ───────── the admin's decision ─────────

/// Longest admin note on a decision.
const MAX_REASON_CHARS: usize = 300;

#[derive(Deserialize)]
pub struct TeachingDecision {
    teacher_id: String,
    subject_id: String,
    status: String,
    /// Optional note (trimmed; empty = none) kept on the row and sent to the teacher with the notification.
    #[serde(default)]
    reason: Option<String>,
}

/// Records the decision and tells the teacher. The wording depends on what the decision undoes: taking away an
/// assignment the teacher HAD (approved -> rejected/pending) sends `teaching_revoked` with the number of items it
/// hid, which is not the same message as turning a first request down (`teaching_rejected`). Repeating a decision
/// that changes nothing (same status, same note) writes and sends nothing.
fn decide_teaching(conn: &Connection, admin: &User, d: &TeachingDecision) -> Res<()> {
    let now = now_ms();
    if !["approved", "rejected", "pending"].contains(&d.status.as_str()) {
        return Err(bad("invalid_status"));
    }
    let reason = opt_text(&d.reason, MAX_REASON_CHARS, "reason_too_long")?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let (previous, previous_reason): (String, Option<String>) = tx
        .query_row(
            "SELECT status, reason FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2",
            params![d.teacher_id, d.subject_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if previous == d.status && previous_reason == reason {
        return Ok(());
    }
    // counted BEFORE the change: afterwards nothing of it is visible any more
    let hidden = if previous == "approved" && d.status != "approved" { teaching_impact(&tx, &d.teacher_id, &d.subject_id, now)?.total() } else { 0 };
    // a request that is waiting again has no decision (and no note) yet
    let decided_at = (d.status != "pending").then_some(now);
    tx.execute(
        "UPDATE teacher_subjects SET status = ?3, decided_at = ?4, reason = ?5 WHERE teacher_id = ?1 AND subject_id = ?2",
        params![d.teacher_id, d.subject_id, d.status, decided_at, reason],
    )
    .map_err(db_err)?;
    audit(&tx, &admin.id, &d.teacher_id, "teaching_decided", &format!("{} -> {}", d.subject_id, d.status));
    let subject: String = tx
        .query_row("SELECT name_ar FROM subjects WHERE id = ?1", params![d.subject_id], |r| r.get(0))
        .unwrap_or_default();
    let mut data = serde_json::json!({ "subject": subject });
    if let Some(note) = &reason {
        data["reason"] = serde_json::Value::String(note.clone());
    }
    match (previous.as_str(), d.status.as_str()) {
        ("approved", "rejected" | "pending") => {
            data["hidden"] = hidden.into();
            crate::platform_engage::notify(&tx, &d.teacher_id, "teaching_revoked", data, "/platform/teaching");
        }
        (_, "approved") => crate::platform_engage::notify(&tx, &d.teacher_id, "teaching_approved", data, &format!("/platform/subjects/{}", d.subject_id)),
        (_, "rejected") => crate::platform_engage::notify(&tx, &d.teacher_id, "teaching_rejected", data, "/platform/teaching"),
        _ => {} // back to pending from rejected: the request is simply waiting again
    }
    tx.commit().map_err(db_err)
}

pub async fn admin_decide_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(d): Json<TeachingDecision>,
) -> Res<StatusCode> {
    let admin = require_admin(&state, &headers)?;
    decide_teaching(&*lock(&state)?, &admin, &d)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── student placement / enrollment ─────────

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Placement {
    institution_id: String,
    unit_id: Option<String>,
}

fn set_placement(conn: &Connection, student: &User, p: &Placement) -> Res<()> {
    let ok: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM institutions WHERE id = ?1 AND is_active = 1)", params![p.institution_id], |r| r.get(0))
        .map_err(db_err)?;
    if !ok {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    if let Some(uid) = &p.unit_id {
        let same: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM org_units WHERE id = ?1 AND institution_id = ?2)",
                params![uid, p.institution_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if !same {
            return Err(bad("unit_other_institution"));
        }
    }
    conn.execute(
        "INSERT INTO student_placement(student_id, institution_id, unit_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(student_id) DO UPDATE SET institution_id = excluded.institution_id, unit_id = excluded.unit_id",
        params![student.id, p.institution_id, p.unit_id],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn get_placement_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Option<Placement>>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    conn.query_row(
        "SELECT institution_id, unit_id FROM student_placement WHERE student_id = ?1",
        params![user.id],
        |r| Ok(Placement { institution_id: r.get(0)?, unit_id: r.get(1)? }),
    )
    .optional()
    .map_err(db_err)
    .map(Json)
}

pub async fn set_placement_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(p): Json<Placement>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    set_placement(&*lock(&state)?, &user, &p)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Debug)]
pub struct EnrolledSubject {
    subject_id: String,
    subject_name: String,
    institution_id: String,
    institution_name: String,
}

fn my_enrollments(conn: &Connection, student_id: &str) -> Res<Vec<EnrolledSubject>> {
    conn.prepare(
        "SELECT s.id, s.name_ar, i.id, i.name_ar FROM subject_enrollments e
         JOIN subjects s ON s.id = e.subject_id JOIN institutions i ON i.id = s.institution_id
         WHERE e.student_id = ?1 ORDER BY e.created_at DESC",
    )
    .map_err(db_err)?
    .query_map(params![student_id], |r| {
        Ok(EnrolledSubject { subject_id: r.get(0)?, subject_name: r.get(1)?, institution_id: r.get(2)?, institution_name: r.get(3)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

pub async fn my_enrollments_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<EnrolledSubject>>> {
    let user = require_role(&state, &headers, "student")?;
    my_enrollments(&*lock(&state)?, &user.id).map(Json)
}

fn enroll(conn: &Connection, student: &User, subject_id: &str) -> Res<()> {
    let active: Option<bool> = conn
        .query_row(
            "SELECT s.is_active AND i.is_active FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if active != Some(true) {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, ?2, ?3)",
        params![student.id, subject_id, now_ms()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn enroll_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SubjectRef>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    enroll(&*lock(&state)?, &user, &req.subject_id)?;
    Ok(StatusCode::CREATED)
}

pub async fn unenroll_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    lock(&state)?
        .execute("DELETE FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2", params![user.id, subject_id])
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── follows ─────────

fn follow(conn: &Connection, student: &User, teacher_id: &str) -> Res<()> {
    let is_teacher: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1 AND role = 'teacher' AND status = 'active')",
            params![teacher_id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if !is_teacher {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO teacher_follows(student_id, teacher_id, created_at) VALUES (?1, ?2, ?3)",
        params![student.id, teacher_id, now_ms()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn follow_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    follow(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfollow_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    lock(&state)?
        .execute("DELETE FROM teacher_follows WHERE student_id = ?1 AND teacher_id = ?2", params![user.id, id])
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── public pages ─────────

#[derive(Serialize, Debug)]
pub struct TeacherCard {
    id: String,
    full_name: String,
    bio: Option<String>,
    subject_count: i64,
}

#[derive(Deserialize)]
pub struct TeacherQuery {
    q: Option<String>,
    subject_id: Option<String>,
}

/// Teachers for the public directory. `only_active`: count and match only the approved subjects students can reach
/// (the subject AND its institution switched on); an admin looking at a hidden subject passes `false` to see who is
/// attached to it.
fn list_teachers_in(conn: &Connection, q: &TeacherQuery, only_active: bool) -> Res<Vec<TeacherCard>> {
    let like = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(like_pattern);
    let reachable = if only_active { format!("AND {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE}") } else { String::new() };
    conn.prepare(&format!(
        "SELECT u.id, u.full_name, u.bio,
                (SELECT count(*) FROM teacher_subjects t JOIN subjects s ON s.id = t.subject_id
                  WHERE t.teacher_id = u.id AND t.status = 'approved' {reachable})
         FROM users u
         WHERE u.role = 'teacher' AND u.status = 'active'
           AND (?1 IS NULL OR lower(u.full_name) LIKE ?1 ESCAPE '\\')
           AND (?2 IS NULL OR EXISTS(SELECT 1 FROM teacher_subjects t JOIN subjects s ON s.id = t.subject_id
                                      WHERE t.teacher_id = u.id AND t.subject_id = ?2 AND t.status = 'approved' {reachable}))
         ORDER BY u.full_name LIMIT 100"
    ))
    .map_err(db_err)?
    .query_map(params![like, q.subject_id], |r| {
        Ok(TeacherCard { id: r.get(0)?, full_name: r.get(1)?, bio: r.get(2)?, subject_count: r.get(3)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

fn list_teachers(conn: &Connection, q: &TeacherQuery) -> Res<Vec<TeacherCard>> {
    list_teachers_in(conn, q, true)
}

pub async fn list_teachers_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TeacherQuery>,
) -> Res<Json<Vec<TeacherCard>>> {
    require_active(&state, &headers)?;
    list_teachers(&*lock(&state)?, &q).map(Json)
}

#[derive(Serialize, Debug)]
pub struct TeacherPage {
    id: String,
    full_name: String,
    bio: Option<String>,
    followers: i64,
    following: bool,
    subjects: Vec<Teaching>,
}

/// The subjects a teacher is approved for AND students can reach (subject and institution on), as the public teacher
/// page lists them: the plain [`Teaching`] fields only.
fn public_subjects(conn: &Connection, teacher_id: &str) -> Res<Vec<Teaching>> {
    conn.prepare(&format!(
        "{} WHERE ts.teacher_id = ?1 AND ts.status = 'approved' AND {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE} ORDER BY i.name_ar, s.name_ar",
        teaching_select("")
    ))
    .map_err(db_err)?
    .query_map(params![teacher_id], map_teaching)
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

fn teacher_page(conn: &Connection, viewer: &User, id: &str) -> Res<TeacherPage> {
    let (full_name, bio): (String, Option<String>) = conn
        .query_row(
            "SELECT full_name, bio FROM users WHERE id = ?1 AND role = 'teacher' AND status = 'active'",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let followers: i64 = conn
        .query_row("SELECT count(*) FROM teacher_follows WHERE teacher_id = ?1", params![id], |r| r.get(0))
        .map_err(db_err)?;
    let following: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM teacher_follows WHERE teacher_id = ?1 AND student_id = ?2)",
            params![id, viewer.id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    let subjects = public_subjects(conn, id)?;
    Ok(TeacherPage { id: id.to_string(), full_name, bio, followers, following, subjects })
}

pub async fn teacher_page_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Json<TeacherPage>> {
    let viewer = require_active(&state, &headers)?;
    teacher_page(&*lock(&state)?, &viewer, &id).map(Json)
}

#[derive(Serialize)]
pub struct SubjectPage {
    id: String,
    name_ar: String,
    name_en: Option<String>,
    institution_id: String,
    institution_name: String,
    path: Vec<String>,
    enrolled: bool,
    enrolled_count: i64,
    teachers: Vec<TeacherCard>,
}

pub async fn subject_page_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Json<SubjectPage>> {
    let viewer = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let (name_ar, name_en, institution_id, institution_name, unit_id, active): (String, Option<String>, String, String, Option<String>, bool) = conn
        .query_row(
            "SELECT s.name_ar, s.name_en, i.id, i.name_ar, s.unit_id, s.is_active AND i.is_active
             FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if !active && viewer.role != "admin" {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    // Names of the unit chain, root first (e.g. department > level > term).
    let path = unit_path(&conn, unit_id)?;
    let enrolled: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2)", params![viewer.id, id], |r| r.get(0))
        .map_err(db_err)?;
    let enrolled_count: i64 = conn
        .query_row("SELECT count(*) FROM subject_enrollments WHERE subject_id = ?1", params![id], |r| r.get(0))
        .map_err(db_err)?;
    let teachers = list_teachers_in(&conn, &TeacherQuery { q: None, subject_id: Some(id.clone()) }, viewer.role != "admin")?;
    Ok(Json(SubjectPage { id, name_ar, name_en, institution_id, institution_name, path, enrolled, enrolled_count, teachers }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    fn seed(conn: &Connection) -> (String, String) {
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i2','school','م',1,0)", []).unwrap();
        conn.execute("INSERT INTO org_units(id,institution_id,kind,name_ar,created_at) VALUES ('u1','i1','level','L1',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,unit_id,name_ar,is_active,created_at) VALUES ('s1','i1','u1','برمجة',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s2','i2','رياضيات',0,0)", []).unwrap();
        ("s1".into(), "s2".into())
    }

    /// The admin queue filtered by one assignment status.
    fn queue(status: &str) -> AdminTeachingQuery {
        AdminTeachingQuery { status: Some(status.into()), ..Default::default() }
    }

    #[test]
    fn teaching_requires_approval_and_flows() {
        let conn = create_test_db();
        let (s1, s2) = seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        assert_eq!(request_teaching(&conn, &t, &s2).unwrap_err().0, StatusCode::NOT_FOUND, "inactive subject");
        assert_eq!(request_teaching(&conn, &t, "nope").unwrap_err().0, StatusCode::NOT_FOUND);
        request_teaching(&conn, &t, &s1).unwrap();
        assert_eq!(request_teaching(&conn, &t, &s1).unwrap_err().0, StatusCode::CONFLICT);
        // pending teachers do not show up for the subject
        let q = TeacherQuery { q: None, subject_id: Some(s1.clone()) };
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 0);
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "approved".into(), reason: None }).unwrap();
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 1);
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "rejected".into(), reason: None }).unwrap();
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 0);
        request_teaching(&conn, &t, &s1).unwrap(); // re-request after rejection
        assert_eq!(my_teaching(&conn, &t.id).unwrap()[0].base.status, "pending");
        let bad_status = TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "x".into(), reason: None };
        assert_eq!(decide_teaching(&conn, &admin, &bad_status).unwrap_err().0, StatusCode::BAD_REQUEST);
        let missing = TeachingDecision { teacher_id: "no".into(), subject_id: s1, status: "approved".into(), reason: None };
        assert_eq!(decide_teaching(&conn, &admin, &missing).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn placement_validates_unit_belongs_to_institution() {
        let conn = create_test_db();
        seed(&conn);
        let s = insert_test_user(&conn, "s@x.com", "student", "active");
        let p = |i: &str, u: Option<&str>| Placement { institution_id: i.into(), unit_id: u.map(Into::into) };
        set_placement(&conn, &s, &p("i1", Some("u1"))).unwrap();
        assert_eq!(set_placement(&conn, &s, &p("i2", Some("u1"))).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(set_placement(&conn, &s, &p("zzz", None)).unwrap_err().0, StatusCode::NOT_FOUND);
        set_placement(&conn, &s, &p("i2", None)).unwrap(); // update in place
        let n: i64 = conn.query_row("SELECT count(*) FROM student_placement", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn enroll_and_follow_rules() {
        let conn = create_test_db();
        let (s1, s2) = seed(&conn);
        let stu = insert_test_user(&conn, "s@x.com", "student", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let pending_t = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        enroll(&conn, &stu, &s1).unwrap();
        enroll(&conn, &stu, &s1).unwrap(); // idempotent
        assert_eq!(enroll(&conn, &stu, &s2).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(my_enrollments(&conn, &stu.id).unwrap().len(), 1);
        follow(&conn, &stu, &t.id).unwrap();
        assert_eq!(follow(&conn, &stu, &pending_t.id).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(follow(&conn, &stu, &stu.id).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn profile_update_validates() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        update_profile(&conn, &t, &ProfilePatch { full_name: Some(" د. أحمد ".into()), bio: Some("خبير".into()), ..Default::default() }).unwrap();
        let card = &list_teachers(&conn, &TeacherQuery { q: Some("أحمد".into()), subject_id: None }).unwrap()[0];
        assert_eq!((card.full_name.as_str(), card.bio.as_deref()), ("د. أحمد", Some("خبير")));
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { full_name: Some("  ".into()), ..Default::default() }).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { bio: Some("x".repeat(1001)), ..Default::default() }).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    // ───── phase 3-1: kind of institution in the profile ─────

    fn inst_of(conn: &Connection, u: &User) -> Option<String> {
        conn.query_row("SELECT institution_type FROM users WHERE id = ?1", params![u.id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_profile_accepts_only_known_institution_types_and_only_changes_the_callers_row() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let bystander = insert_test_user(&conn, "b@x.com", "teacher", "active");
        let set = |v: &str| ProfilePatch { institution_type: Some(v.into()), ..Default::default() };
        for ok in ["school", "institute", "university"] {
            update_profile(&conn, &t, &set(ok)).unwrap();
            assert_eq!(inst_of(&conn, &t).as_deref(), Some(ok));
        }
        for bad_value in ["college", "", " school", "SCHOOL", "university ", "<script>"] {
            let e = update_profile(&conn, &t, &set(bad_value)).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_type")), (StatusCode::BAD_REQUEST, true), "{bad_value:?}");
        }
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"), "refused values change nothing");
        assert_eq!(inst_of(&conn, &bystander), None, "another user's row is never touched");
        // omitted / null leaves it alone
        update_profile(&conn, &t, &ProfilePatch { bio: Some("نبذة".into()), ..Default::default() }).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"));
        let from_json: ProfilePatch = serde_json::from_value(serde_json::json!({"institution_type": null})).unwrap();
        update_profile(&conn, &t, &from_json).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"));
        let from_json: ProfilePatch = serde_json::from_value(serde_json::json!({"institution_type": "school"})).unwrap();
        update_profile(&conn, &t, &from_json).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("school"));
    }

    #[test]
    fn a_bad_field_leaves_the_whole_profile_patch_unapplied() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        update_profile(&conn, &t, &ProfilePatch { bio: Some("قديمة".into()), institution_type: Some("school".into()), ..Default::default() }).unwrap();
        let name = |c: &Connection| -> String { c.query_row("SELECT full_name FROM users WHERE id = ?1", params![t.id], |r| r.get(0)).unwrap() };
        let bio = |c: &Connection| -> Option<String> { c.query_row("SELECT bio FROM users WHERE id = ?1", params![t.id], |r| r.get(0)).unwrap() };
        for patch in [
            ProfilePatch { full_name: Some("اسم جديد".into()), institution_type: Some("bogus".into()), ..Default::default() },
            ProfilePatch { full_name: Some("اسم جديد".into()), bio: Some("x".repeat(1001)), ..Default::default() },
            ProfilePatch { bio: Some("جديدة".into()), institution_type: Some("bogus".into()), ..Default::default() },
        ] {
            assert!(update_profile(&conn, &t, &patch).is_err());
            assert_eq!((name(&conn).as_str(), bio(&conn).as_deref(), inst_of(&conn, &t).as_deref()), ("T", Some("قديمة"), Some("school")), "nothing half-applied");
        }
    }

    // ───── phase 3-1: a teacher waiting for approval can already ask for subjects ─────

    fn reload(conn: &Connection, u: &User) -> User {
        let status: String = conn.query_row("SELECT status FROM users WHERE id = ?1", params![u.id], |r| r.get(0)).unwrap();
        User { status, ..u.clone() }
    }

    fn visible_posts(conn: &Connection) -> i64 {
        conn.query_row(
            &format!(
                "SELECT count(*) FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id WHERE {}",
                crate::platform_content::visible("p", "published")
            ),
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn only_active_and_pending_teachers_may_request_a_subject() {
        let conn = create_test_db();
        let who = |role: &str, status: &str| insert_test_user(&conn, &format!("{role}-{status}@x.com"), role, status);
        for (role, status, allowed) in [
            ("teacher", "active", true),
            ("teacher", "pending", true),
            ("teacher", "rejected", false),
            ("teacher", "suspended", false),
            ("student", "active", false),
            ("student", "pending", false),
            ("admin", "active", false),
        ] {
            let r = may_request_teaching(who(role, status));
            assert_eq!(r.is_ok(), allowed, "{role}/{status}");
            if !allowed {
                assert_eq!(r.unwrap_err().0, StatusCode::FORBIDDEN);
            }
        }
    }

    #[test]
    fn a_pending_teacher_can_browse_the_catalogue_and_manage_requests_and_nobody_else_gains_anything() {
        let conn = create_test_db();
        let who = |role: &str, status: &str| insert_test_user(&conn, &format!("{role}-{status}@x.com"), role, status);
        // (role, status, may browse institutions/structure, may ask for / list / withdraw subject requests)
        let matrix = [
            ("admin", "active", true, false),
            ("student", "active", true, false),
            ("teacher", "active", true, true),
            ("teacher", "pending", true, true), // the point of the phase: a pending teacher can find a subject and ask for it
            ("student", "pending", false, false),
            ("teacher", "rejected", false, false),
            ("teacher", "suspended", false, false),
            ("student", "suspended", false, false),
        ];
        for (role, status, browse, teach) in matrix {
            let u = who(role, status);
            assert_eq!(may_browse_catalog(&u), browse, "browse {role}/{status}");
            assert_eq!(may_request_teaching(u).is_ok(), teach, "teaching flow {role}/{status}");
        }
        // everything else a pending teacher might try still needs an ACTIVE account
        let pending = insert_test_user(&conn, "the-pending-one@x.com", "teacher", "pending");
        for denied in [
            crate::platform::ensure_role(pending.clone(), "teacher").map(|_| ()),
            crate::platform::ensure_active(pending.clone()).map(|_| ()),
            crate::platform::ensure_role(pending.clone(), "admin").map(|_| ()),
        ] {
            assert_eq!(denied.unwrap_err().0, StatusCode::FORBIDDEN);
        }
        // and the flow itself works end to end on the data layer: ask, see it, withdraw it
        let (s1, _) = seed(&conn);
        request_teaching(&conn, &pending, &s1).unwrap();
        assert_eq!(my_teaching(&conn, &pending.id).unwrap().len(), 1);
        conn.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2", params![pending.id, s1]).unwrap();
        assert!(my_teaching(&conn, &pending.id).unwrap().is_empty());
    }

    #[test]
    fn a_pending_teacher_can_request_a_subject_but_gets_nothing_else_until_approved() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        enroll(&conn, &student, &s1).unwrap();

        // the request goes through and lands in the admin's queue
        let me = may_request_teaching(pending.clone()).unwrap();
        request_teaching(&conn, &me, &s1).unwrap();
        assert_eq!(request_teaching(&conn, &me, &s1).unwrap_err().0, StatusCode::CONFLICT, "still one request per subject");

        // …but everything else a teacher does keeps demanding an active account (the handlers' `require_role` guard)
        assert_eq!(crate::platform::ensure_role(pending.clone(), "teacher").unwrap_err().0, StatusCode::FORBIDDEN, "create posts/courses/live/exams, read my teaching, …");
        assert_eq!(crate::platform::ensure_active(pending.clone()).unwrap_err().0, StatusCode::FORBIDDEN);

        // approving the ASSIGNMENT does not activate the ACCOUNT
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: s1.clone(), status: "approved".into(), reason: None }).unwrap();
        let after = reload(&conn, &pending);
        assert_eq!(after.status, "pending", "the account is still waiting");
        assert_eq!(crate::platform::ensure_role(after, "teacher").unwrap_err().0, StatusCode::FORBIDDEN);

        // content of a pending teacher stays invisible even with an approved assignment: posts and exams alike
        conn.execute("INSERT INTO posts VALUES ('p1', ?1, 's1', 'article', 'منشور', 'نص', 'published', NULL, 0, 0)", params![pending.id]).unwrap();
        conn.execute(
            "INSERT INTO assessments(id,teacher_id,subject_id,title,questions,question_count,total_points,status,created_at,updated_at) VALUES ('e1',?1,'s1','امتحان','[]',1,1,'published',0,0)",
            params![pending.id],
        )
        .unwrap();
        let available = |c: &Connection| crate::platform_exams::list_available(c, &student.id).unwrap().len();
        assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "account pending, assignment approved: invisible");

        // active account but assignment not (or no longer) approved: invisible
        conn.execute("UPDATE users SET status = 'active' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!((visible_posts(&conn), available(&conn)), (1, 1), "both conditions hold: visible");
        for assignment in ["pending", "rejected"] {
            conn.execute("UPDATE teacher_subjects SET status = ?1 WHERE teacher_id = ?2", params![assignment, pending.id]).unwrap();
            assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "assignment {assignment}: invisible");
        }
        conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![pending.id]).unwrap();
        conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "suspended account: invisible again");
    }

    #[test]
    fn the_admin_queue_shows_the_state_of_the_teachers_account_next_to_each_request() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let active = insert_test_user(&conn, "t@x.com", "teacher", "active");
        request_teaching(&conn, &pending, &s1).unwrap();
        request_teaching(&conn, &active, &s1).unwrap();
        let rows = admin_teaching(&conn, &AdminTeachingQuery::default()).unwrap().items;
        assert_eq!(rows.len(), 2);
        let status_of = |id: &str| rows.iter().find(|r| r.base.teacher_id == id).map(|r| r.teacher_status.clone()).unwrap();
        assert_eq!((status_of(&pending.id).as_str(), status_of(&active.id).as_str()), ("pending", "active"));
        // the JSON keeps every field the page already used and adds teacher_status
        let json = serde_json::to_value(rows.iter().find(|r| r.base.teacher_id == pending.id).unwrap()).unwrap();
        for key in ["teacher_id", "teacher_name", "subject_id", "subject_name", "institution_id", "institution_name", "status", "teacher_status"] {
            assert!(json.get(key).is_some(), "{key} in {json}");
        }
        assert_eq!((json["status"].as_str(), json["teacher_status"].as_str()), (Some("pending"), Some("pending")), "assignment status and account status are different fields");
        // the status filter still filters assignments
        conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![active.id]).unwrap();
        assert_eq!(admin_teaching(&conn, &queue("approved")).unwrap().items.len(), 1);
        assert_eq!(admin_teaching(&conn, &queue("pending")).unwrap().items.len(), 1);
        // an account the admin later rejects keeps showing its (rejected) account state
        conn.execute("UPDATE users SET status = 'rejected' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!(admin_teaching(&conn, &queue("pending")).unwrap().items[0].teacher_status, "rejected");
    }

    fn extra_subjects(conn: &Connection, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                let id = format!("x{i}");
                conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES (?1,'i1','مادة',1,0)", params![id]).unwrap();
                id
            })
            .collect()
    }

    #[test]
    fn a_teacher_can_only_have_a_limited_number_of_requests_waiting() {
        let conn = create_test_db();
        seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        let subjects = extra_subjects(&conn, MAX_OPEN_TEACHING_REQUESTS as usize + 2);
        let (first_batch, rest) = subjects.split_at(MAX_OPEN_TEACHING_REQUESTS as usize);
        for s in first_batch {
            request_teaching(&conn, &pending, s).unwrap();
        }
        let waiting = |c: &Connection, t: &User| -> i64 { c.query_row("SELECT count(*) FROM teacher_subjects WHERE teacher_id = ?1 AND status = 'pending'", params![t.id], |r| r.get(0)).unwrap() };
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);

        // the next one is refused with 429 and nothing is written
        let e = request_teaching(&conn, &pending, &rest[0]).unwrap_err();
        assert_eq!((e.0, e.1.contains("too_many_pending_requests")), (StatusCode::TOO_MANY_REQUESTS, true));
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);
        // asking again for one that is already waiting is still reported as that, not as the cap
        assert_eq!(request_teaching(&conn, &pending, &first_batch[0]).unwrap_err().0, StatusCode::CONFLICT);
        // the cap belongs to the teacher, not to the platform
        request_teaching(&conn, &other, &rest[0]).unwrap();

        // a rejected request that is retried would be a new waiting one, so it is capped too
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: first_batch[0].clone(), status: "rejected".into(), reason: None }).unwrap();
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS - 1);
        request_teaching(&conn, &pending, &rest[0]).unwrap(); // a slot was freed by the decision
        assert_eq!(request_teaching(&conn, &pending, &first_batch[0]).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS, "retrying the rejected one needs a free slot");
        // approving frees a slot as well (an approved assignment is not "waiting")
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: first_batch[1].clone(), status: "approved".into(), reason: None }).unwrap();
        request_teaching(&conn, &pending, &first_batch[0]).unwrap();
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);
    }

    #[test]
    fn the_admin_queue_lists_requests_of_active_accounts_before_those_of_unapproved_ones() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let others = extra_subjects(&conn, 1);
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let active = insert_test_user(&conn, "t@x.com", "teacher", "active");
        request_teaching(&conn, &active, &s1).unwrap();
        request_teaching(&conn, &pending, &s1).unwrap();
        request_teaching(&conn, &pending, &others[0]).unwrap();
        // the unapproved account's requests are the newest ones…
        conn.execute("UPDATE teacher_subjects SET created_at = 1 WHERE teacher_id = ?1", params![active.id]).unwrap();
        conn.execute("UPDATE teacher_subjects SET created_at = 9000000000000 WHERE teacher_id = ?1", params![pending.id]).unwrap();
        let rows = admin_teaching(&conn, &queue("pending")).unwrap().items;
        let order: Vec<&str> = rows.iter().map(|r| r.teacher_status.as_str()).collect();
        // …and still come last, so they can never push a real teacher's request off the page
        assert_eq!(order, ["active", "pending", "pending"]);
    }

    #[test]
    fn deleting_a_subject_cascades_assignments() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let stu = insert_test_user(&conn, "s@x.com", "student", "active");
        request_teaching(&conn, &t, &s1).unwrap();
        enroll(&conn, &stu, &s1).unwrap();
        conn.execute("DELETE FROM institutions WHERE id = 'i1'", []).unwrap();
        let n: i64 = conn.query_row("SELECT (SELECT count(*) FROM teacher_subjects)+(SELECT count(*) FROM subject_enrollments)", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    // ───── phase 3-2: the teacher's own view of an assignment, the admin's note, what losing one hides ─────

    /// i1 (active) with a department > level chain holding subject s1, a second subject s2 directly under the
    /// institution, and a second institution i2 with s3.
    fn campus() -> Connection {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0), ('i2','school','مدرسة',1,0);
             INSERT INTO org_units(id,institution_id,parent_id,kind,name_ar,created_at) VALUES ('d1','i1',NULL,'department','كلية الحاسوب',0);
             INSERT INTO org_units(id,institution_id,parent_id,kind,name_ar,created_at) VALUES ('l1','i1','d1','level','المستوى الأول',0);
             INSERT INTO subjects(id,institution_id,unit_id,name_ar,name_en,is_active,created_at) VALUES ('s1','i1','l1','برمجة','Programming',1,0);
             INSERT INTO subjects(id,institution_id,unit_id,name_ar,is_active,created_at) VALUES ('s2','i1',NULL,'ثقافة',1,0);
             INSERT INTO subjects(id,institution_id,unit_id,name_ar,is_active,created_at) VALUES ('s3','i2',NULL,'رياضيات',1,0);",
        )
        .unwrap();
        conn
    }

    fn decide(conn: &Connection, admin: &User, teacher: &User, subject: &str, status: &str, reason: Option<&str>) -> Res<()> {
        decide_teaching(conn, admin, &TeachingDecision { teacher_id: teacher.id.clone(), subject_id: subject.into(), status: status.into(), reason: reason.map(Into::into) })
    }

    fn notifications(conn: &Connection, user: &User) -> Vec<(String, serde_json::Value, Option<String>)> {
        conn.prepare("SELECT kind, data, link FROM notifications WHERE user_id = ?1 ORDER BY rowid")
            .unwrap()
            .query_map(params![user.id], |r| Ok((r.get(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap(), r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn keys(v: &serde_json::Value) -> std::collections::BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    fn raw_item(conn: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) {
        conn.execute(sql, args).unwrap();
    }

    fn put_post(conn: &Connection, id: &str, teacher: &User, subject: &str, status: &str) {
        raw_item(conn, "INSERT INTO posts VALUES (?1, ?2, ?3, 'article', 'منشور', 'نص', ?4, NULL, 0, 0)", &[&id, &teacher.id, &subject, &status]);
    }

    fn put_course(conn: &Connection, id: &str, teacher: &User, subject: &str, status: &str) {
        raw_item(conn, "INSERT INTO courses(id,teacher_id,subject_id,title,description,status,created_at,updated_at) VALUES (?1,?2,?3,'دورة',NULL,?4,0,0)", &[&id, &teacher.id, &subject, &status]);
    }

    fn put_live(conn: &Connection, id: &str, teacher: &User, subject: &str, status: &str, starts_at: i64, duration: i64) {
        raw_item(
            conn,
            "INSERT INTO live_sessions(id,teacher_id,subject_id,title,description,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,?3,'بث',NULL,?4,?5,'https://m.example.com/x',?6,0)",
            &[&id, &teacher.id, &subject, &starts_at, &duration, &status],
        );
    }

    fn put_exam(conn: &Connection, id: &str, teacher: &User, subject: &str, status: &str) {
        raw_item(
            conn,
            "INSERT INTO assessments(id,teacher_id,subject_id,title,questions,question_count,total_points,status,created_at,updated_at) VALUES (?1,?2,?3,'امتحان','[]',1,1,?4,0,0)",
            &[&id, &teacher.id, &subject, &status],
        );
    }

    #[test]
    fn the_teachers_own_rows_carry_dates_note_unit_path_and_flags_but_the_public_page_never_does() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        request_teaching(&conn, &t, "s1").unwrap();
        request_teaching(&conn, &t, "s2").unwrap();
        request_teaching(&conn, &other, "s3").unwrap();
        let before = now_ms();

        let rows = my_teaching(&conn, &t.id).unwrap();
        assert_eq!(rows.len(), 2, "another teacher's request is not mine");
        let json: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
        // the ordering is by institution, then subject name; both are in i1
        let s1 = json.iter().find(|r| r["subject_id"] == "s1").unwrap();
        let s2 = json.iter().find(|r| r["subject_id"] == "s2").unwrap();
        let expected: std::collections::BTreeSet<String> = [
            "teacher_id", "teacher_name", "subject_id", "subject_name", "institution_id", "institution_name", "status", // everything the page already used
            "created_at", "decided_at", "reason", "unit_path", "subject_active", "institution_active",
        ]
        .iter()
        .map(|k| k.to_string())
        .collect();
        assert_eq!(keys(s1), expected);
        assert_eq!(s1["unit_path"], serde_json::json!(["كلية الحاسوب", "المستوى الأول"]), "root first, leaf last");
        assert_eq!(s2["unit_path"], serde_json::json!([]), "a subject on the institution itself has no path");
        assert!(s1["created_at"].as_i64().unwrap() <= before && s1["created_at"].as_i64().unwrap() > 0);
        assert!(s1["decided_at"].is_null() && s1["reason"].is_null());
        assert_eq!((&s1["subject_active"], &s1["institution_active"], &s1["status"]), (&serde_json::json!(true), &serde_json::json!(true), &serde_json::json!("pending")));

        decide(&conn, &admin, &t, "s1", "approved", Some("  أهلاً بك  ")).unwrap();
        let s1 = serde_json::to_value(&my_teaching(&conn, &t.id).unwrap().into_iter().find(|r| r.base.subject_id == "s1").unwrap()).unwrap();
        assert_eq!((s1["status"].as_str(), s1["reason"].as_str()), (Some("approved"), Some("أهلاً بك")));
        assert!(s1["decided_at"].as_i64().unwrap() >= before);

        // the flags follow the subject and the institution
        conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's2'", []).unwrap();
        conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        let flags: Vec<(String, bool, bool)> = my_teaching(&conn, &t.id).unwrap().into_iter().map(|r| (r.base.subject_id, r.extra.subject_active, r.extra.institution_active)).collect();
        assert!(flags.contains(&("s1".into(), true, false)) && flags.contains(&("s2".into(), false, false)), "{flags:?}");
        conn.execute_batch("UPDATE subjects SET is_active = 1; UPDATE institutions SET is_active = 1").unwrap();

        // the PUBLIC page lists only the plain fields: no dates, no note, no path, no flags
        let page = teacher_page(&conn, &student, &t.id).unwrap();
        let page_json = serde_json::to_value(&page).unwrap();
        assert_eq!(page.subjects.len(), 1, "only the approved one");
        let public_keys = keys(&page_json["subjects"][0]);
        assert_eq!(public_keys, ["teacher_id", "teacher_name", "subject_id", "subject_name", "institution_id", "institution_name", "status"].iter().map(|k| k.to_string()).collect());
        for private in ["created_at", "decided_at", "reason", "unit_path", "subject_active", "institution_active"] {
            assert!(!public_keys.contains(private), "{private} must stay private");
        }
        assert!(!page_json.to_string().contains("أهلاً بك"), "the admin's note is not on the public page: {page_json}");
    }

    #[test]
    fn the_admin_note_is_trimmed_bounded_kept_with_the_decision_and_cleared_by_a_new_request() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        request_teaching(&conn, &t, "s1").unwrap();
        let row = |c: &Connection| -> (String, Option<i64>, Option<String>) {
            c.query_row("SELECT status, decided_at, reason FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = 's1'", params![t.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
        };
        let json: TeachingDecision = serde_json::from_value(serde_json::json!({"teacher_id": t.id, "subject_id": "s1", "status": "rejected"})).unwrap();
        assert!(json.reason.is_none(), "the field is optional on the wire");
        decide_teaching(&conn, &admin, &json).unwrap();
        assert_eq!(row(&conn).2, None);

        decide(&conn, &admin, &t, "s1", "rejected", Some("  المحتوى غير مكتمل  ")).unwrap();
        let (status, decided, reason) = row(&conn);
        assert_eq!((status.as_str(), reason.as_deref()), ("rejected", Some("المحتوى غير مكتمل")));
        assert!(decided.is_some());
        let sent = notifications(&conn, &t);
        assert_eq!(sent.last().unwrap().0, "teaching_rejected");
        assert_eq!(sent.last().unwrap().1["reason"], "المحتوى غير مكتمل", "the note travels with the notification");
        assert_eq!(sent.last().unwrap().1["subject"], "برمجة");

        // empty means none; exactly 300 characters is fine, 301 is refused and changes nothing
        decide(&conn, &admin, &t, "s1", "rejected", Some("   ")).unwrap();
        assert_eq!(row(&conn).2, None);
        decide(&conn, &admin, &t, "s1", "rejected", Some(&"ع".repeat(300))).unwrap();
        assert_eq!(row(&conn).2.unwrap().chars().count(), 300);
        let e = decide(&conn, &admin, &t, "s1", "approved", Some(&"ع".repeat(301))).unwrap_err();
        assert_eq!((e.0, e.1.contains("reason_too_long")), (StatusCode::BAD_REQUEST, true));
        assert_eq!(row(&conn).0, "rejected", "a refused decision changes nothing");

        // a note belongs to a decision: a request that is waiting again has none, and neither has the retry
        decide(&conn, &admin, &t, "s1", "rejected", Some("سبب")).unwrap();
        decide(&conn, &admin, &t, "s1", "pending", Some("لا يجب أن يبقى")).unwrap();
        let (status, decided, reason) = row(&conn);
        assert_eq!((status.as_str(), decided), ("pending", None), "waiting again: undecided");
        assert_eq!(reason.as_deref(), Some("لا يجب أن يبقى"), "an explicit note given with the decision is stored as given");
        decide(&conn, &admin, &t, "s1", "rejected", Some("سبب جديد")).unwrap();
        request_teaching(&conn, &t, "s1").unwrap();
        assert_eq!(row(&conn), ("pending".into(), None, None), "asking again starts a fresh request");
        // the note never reaches a student
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        assert!(notifications(&conn, &student).is_empty());
    }

    #[test]
    fn taking_away_an_approved_subject_is_told_apart_from_turning_a_request_down() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let kinds = |c: &Connection| notifications(c, &t).into_iter().map(|n| n.0).collect::<Vec<_>>();

        // a first request that is turned down: "rejected"
        request_teaching(&conn, &t, "s1").unwrap();
        decide(&conn, &admin, &t, "s1", "rejected", Some("غير مناسب")).unwrap();
        assert_eq!(kinds(&conn), ["teaching_rejected"]);
        let first = &notifications(&conn, &t)[0];
        assert!(first.1.get("hidden").is_none(), "nothing was hidden by a first rejection");
        assert_eq!(first.2.as_deref(), Some("/platform/teaching"));

        // an approved subject with content
        request_teaching(&conn, &t, "s1").unwrap();
        decide(&conn, &admin, &t, "s1", "approved", None).unwrap();
        assert_eq!(kinds(&conn).last().map(String::as_str), Some("teaching_approved"));
        let now = now_ms();
        put_post(&conn, "p1", &t, "s1", "published");
        put_post(&conn, "p2", &t, "s1", "published");
        put_post(&conn, "p3", &t, "s1", "draft");
        put_course(&conn, "c1", &t, "s1", "published");
        put_live(&conn, "l1", &t, "s1", "scheduled", now + 3_600_000, 60);
        put_live(&conn, "l2", &t, "s1", "scheduled", now - 5 * 3_600_000, 60);
        put_exam(&conn, "e1", &t, "s1", "published");
        put_exam(&conn, "e2", &t, "s1", "draft");
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap(), Impact { posts: 2, courses: 1, live: 1, exams: 1 });

        decide(&conn, &admin, &t, "s1", "rejected", Some("انتهى التعاقد")).unwrap();
        let sent = notifications(&conn, &t);
        let last = sent.last().unwrap();
        assert_eq!(last.0, "teaching_revoked", "approved -> rejected takes something away");
        assert_eq!((last.1["subject"].as_str(), last.1["hidden"].as_i64(), last.1["reason"].as_str()), (Some("برمجة"), Some(5), Some("انتهى التعاقد")));
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap().total(), 0, "and now it really is hidden");

        // approved -> pending is a revocation too; rejected -> pending is only a request waiting again
        decide(&conn, &admin, &t, "s1", "approved", None).unwrap();
        decide(&conn, &admin, &t, "s1", "pending", None).unwrap();
        assert_eq!(kinds(&conn).iter().filter(|k| *k == "teaching_revoked").count(), 2);
        let revoked = notifications(&conn, &t).into_iter().rev().find(|n| n.0 == "teaching_revoked").unwrap();
        assert_eq!(revoked.1["hidden"], 5, "the same five items are hidden again");
        assert!(revoked.1.get("reason").is_none(), "no note, no key");
        decide(&conn, &admin, &t, "s1", "rejected", None).unwrap();
        let total = kinds(&conn).len();
        decide(&conn, &admin, &t, "s1", "pending", None).unwrap();
        assert_eq!(kinds(&conn).len(), total, "rejected -> pending sends nothing");

        // repeating a decision that changes nothing writes and sends nothing
        decide(&conn, &admin, &t, "s1", "approved", Some("x")).unwrap();
        let (n, audits) = (kinds(&conn).len(), conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'teaching_decided'", [], |r| r.get::<_, i64>(0)).unwrap());
        decide(&conn, &admin, &t, "s1", "approved", Some("x")).unwrap();
        assert_eq!(kinds(&conn).len(), n);
        assert_eq!(conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'teaching_decided'", [], |r| r.get::<_, i64>(0)).unwrap(), audits);
        // a missing row is still a 404, an unknown status a 400
        assert_eq!(decide(&conn, &admin, &t, "s2", "approved", None).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(decide(&conn, &admin, &t, "s1", "revoked", None).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_teachers_own_withdrawal_is_audited_with_what_it_hid_and_is_idempotent() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        request_teaching(&conn, &t, "s1").unwrap();
        request_teaching(&conn, &t, "s2").unwrap();
        request_teaching(&conn, &other, "s1").unwrap();
        decide(&conn, &admin, &t, "s1", "approved", None).unwrap();
        put_post(&conn, "p1", &t, "s1", "published");
        put_course(&conn, "c1", &t, "s1", "published");
        put_post(&conn, "p2", &t, "s1", "draft");
        let audit_rows = |c: &Connection| -> Vec<(Option<String>, Option<String>, String)> {
            c.prepare("SELECT actor_id, target_id, detail FROM audit_log WHERE action = 'teaching_withdrawn' ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        withdraw_teaching(&conn, &t, "s1").unwrap();
        let rows = audit_rows(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.as_deref(), rows[0].1.as_deref()), (Some(t.id.as_str()), Some(t.id.as_str())));
        assert!(rows[0].2.contains("s1") && rows[0].2.contains("approved") && rows[0].2.contains("hidden=2"), "{}", rows[0].2);
        assert_eq!(my_teaching(&conn, &t.id).unwrap().len(), 1, "s2 is still asked for");
        let content: i64 = conn.query_row("SELECT (SELECT count(*) FROM posts) + (SELECT count(*) FROM courses)", [], |r| r.get(0)).unwrap();
        assert_eq!(content, 3, "withdrawing hides the content, it does not delete it");
        withdraw_teaching(&conn, &t, "s1").unwrap(); // nothing left to withdraw
        assert_eq!(audit_rows(&conn).len(), 1, "no second audit row");
        withdraw_teaching(&conn, &t, "s2").unwrap(); // a pending request
        let rows = audit_rows(&conn);
        assert!(rows[1].2.contains("pending") && rows[1].2.contains("hidden=0"), "{}", rows[1].2);
        // only the caller's own row is ever touched
        assert_eq!(my_teaching(&conn, &other.id).unwrap().len(), 1);
    }

    #[test]
    fn the_impact_counts_only_what_students_see_now_and_only_the_callers_items_on_that_subject() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        for who in [&t, &other] {
            request_teaching(&conn, who, "s1").unwrap();
            decide(&conn, &admin, who, "s1", "approved", None).unwrap();
        }
        request_teaching(&conn, &t, "s2").unwrap();
        decide(&conn, &admin, &t, "s2", "approved", None).unwrap();
        let now = 1_000_000_000_000i64;
        // visible: 1 post, 1 course, 2 live (one running, one future), 1 exam
        put_post(&conn, "p1", &t, "s1", "published");
        put_course(&conn, "c1", &t, "s1", "published");
        put_live(&conn, "l-running", &t, "s1", "scheduled", now - 30 * 60_000, 60);
        put_live(&conn, "l-future", &t, "s1", "scheduled", now + 60_000, 30);
        put_exam(&conn, "e1", &t, "s1", "published");
        // not counted: drafts, cancelled, over, closed/archived exams, other subject, other teacher
        put_post(&conn, "p-draft", &t, "s1", "draft");
        put_course(&conn, "c-draft", &t, "s1", "draft");
        put_live(&conn, "l-over", &t, "s1", "scheduled", now - 61 * 60_000, 60);
        put_live(&conn, "l-edge", &t, "s1", "scheduled", now - 60 * 60_000, 60); // ends exactly now: over
        put_live(&conn, "l-cancelled", &t, "s1", "cancelled", now + 3_600_000, 60);
        put_exam(&conn, "e-draft", &t, "s1", "draft");
        put_exam(&conn, "e-closed", &t, "s1", "closed");
        put_exam(&conn, "e-archived", &t, "s1", "archived");
        put_post(&conn, "p-s2", &t, "s2", "published");
        put_post(&conn, "p-other", &other, "s1", "published");
        put_exam(&conn, "e-other", &other, "s1", "published");

        let want = Impact { posts: 1, courses: 1, live: 2, exams: 1 };
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap(), want);
        assert_eq!(want.total(), 5);
        assert_eq!(teaching_impact(&conn, &t.id, "s2", now).unwrap(), Impact { posts: 1, ..Default::default() });
        assert_eq!(teaching_impact(&conn, &other.id, "s1", now).unwrap(), Impact { posts: 1, exams: 1, ..Default::default() }, "the other teacher's own items");
        let json = serde_json::to_value(want).unwrap();
        assert_eq!(keys(&json), ["posts", "courses", "live", "exams"].iter().map(|k| k.to_string()).collect());
        assert_eq!(json["live"], 2);

        // the teacher endpoint reads the caller's id from the session and 404s on a row that is not theirs
        assert_eq!(my_impact(&conn, &t, "s1", now).unwrap(), want);
        let stranger = insert_test_user(&conn, "x@x.com", "teacher", "active");
        assert_eq!(my_impact(&conn, &stranger, "s1", now).unwrap_err().0, StatusCode::NOT_FOUND, "no row of their own: nothing is revealed about others");
        assert_eq!(my_impact(&conn, &t, "s3", now).unwrap_err().0, StatusCode::NOT_FOUND);
        assert!(require_assignment_row(&conn, &t.id, "s1").is_ok() && require_assignment_row(&conn, &t.id, "s3").is_err(), "the admin twin shares the same row check");

        // what is hidden already does not count: it would not change
        conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap().total(), 0, "institution off");
        conn.execute_batch("UPDATE institutions SET is_active = 1; UPDATE subjects SET is_active = 0 WHERE id = 's1'").unwrap();
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap().total(), 0, "subject off");
        conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        conn.execute("UPDATE teacher_subjects SET status = 'pending' WHERE teacher_id = ?1 AND subject_id = 's1'", params![t.id]).unwrap();
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap().total(), 0, "not approved");
        conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![t.id]).unwrap();
        assert_eq!(teaching_impact(&conn, &t.id, "s1", now).unwrap().total(), 0, "account off");
    }

    #[test]
    fn a_hidden_institution_leaves_the_teacher_page_and_directory_and_refuses_new_requests() {
        let conn = campus();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let late = insert_test_user(&conn, "l@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        request_teaching(&conn, &t, "s1").unwrap();
        request_teaching(&conn, &t, "s3").unwrap();
        decide(&conn, &admin, &t, "s1", "approved", None).unwrap();
        decide(&conn, &admin, &t, "s3", "approved", None).unwrap();
        let in_subject = |only_active: bool| list_teachers_in(&conn, &TeacherQuery { q: None, subject_id: Some("s1".into()) }, only_active).unwrap();
        let count_of = |c: &Connection| list_teachers(c, &TeacherQuery { q: None, subject_id: None }).unwrap().iter().find(|x| x.id == t.id).map(|x| x.subject_count);
        assert_eq!((teacher_page(&conn, &student, &t.id).unwrap().subjects.len(), in_subject(true).len(), count_of(&conn)), (2, 1, Some(2)));

        conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        let page = teacher_page(&conn, &student, &t.id).unwrap();
        assert_eq!(page.subjects.iter().map(|s| s.subject_id.as_str()).collect::<Vec<_>>(), ["s3"], "the subject of the hidden institution is gone from the page");
        assert!(in_subject(true).is_empty(), "nobody teaches a hidden subject as far as students can tell");
        assert_eq!(in_subject(false).len(), 1, "an admin looking at it still sees who is attached");
        assert_eq!(count_of(&conn), Some(1), "the card counts what students can reach");
        assert_eq!(request_teaching(&conn, &late, "s1").unwrap_err().0, StatusCode::NOT_FOUND, "no new requests into a hidden institution");
        assert_eq!(enroll(&conn, &student, "s1").unwrap_err().0, StatusCode::NOT_FOUND);

        conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's3'", []).unwrap();
        assert_eq!(teacher_page(&conn, &student, &t.id).unwrap().subjects.len(), 1, "a hidden subject is left out as before");
        conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        assert_eq!(teacher_page(&conn, &student, &t.id).unwrap().subjects.len(), 2);
    }

    /// Four teachers asking for subjects; creation times are set explicitly.
    fn queue_fixture() -> (Connection, Vec<User>) {
        let conn = campus();
        let names = [("نور الهدى", "noor@school.edu"), ("Sami Ali", "sami@x.com"), ("100% Teacher", "pct@x.com"), ("منى", "mona@x.com")];
        let mut users = vec![];
        for (i, (name, email)) in names.iter().enumerate() {
            let u = insert_test_user(&conn, email, "teacher", if i == 3 { "pending" } else { "active" });
            conn.execute("UPDATE users SET full_name = ?1 WHERE id = ?2", params![name, u.id]).unwrap();
            users.push(u);
        }
        // (teacher, subject, status, created_at)
        for (t, subject, status, at) in [(0, "s1", "pending", 400), (1, "s1", "pending", 100), (2, "s2", "pending", 300), (3, "s1", "pending", 50), (0, "s2", "approved", 200), (1, "s3", "rejected", 150)] {
            conn.execute(
                "INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at, reason) VALUES (?1,?2,?3,?4,?5,?6)",
                params![users[t].id, subject, status, at, if status == "pending" { None } else { Some(at + 1) }, if status == "rejected" { Some("سبب") } else { None }],
            )
            .unwrap();
        }
        (conn, users)
    }

    fn list(conn: &Connection, f: impl FnOnce(&mut AdminTeachingQuery)) -> AdminTeachingList {
        let mut q = AdminTeachingQuery::default();
        f(&mut q);
        admin_teaching(conn, &q).unwrap()
    }

    fn who(l: &AdminTeachingList) -> Vec<(String, String)> {
        l.items.iter().map(|i| (i.base.teacher_name.clone(), i.base.subject_id.clone())).collect()
    }

    #[test]
    fn the_admin_queue_has_tabs_search_paging_a_total_and_shows_the_oldest_request_first() {
        let (conn, users) = queue_fixture();
        let all = list(&conn, |_| {});
        assert_eq!(all.total, 6);
        // active accounts first (oldest first among them), the unapproved account's request last
        assert_eq!(all.items.last().unwrap().teacher_status, "pending");
        let created: Vec<i64> = all.items.iter().map(|i| i.extra.created_at).collect();
        assert_eq!(created, [100, 150, 200, 300, 400, 50]);

        // tabs
        let pending = list(&conn, |q| q.status = Some("pending".into()));
        assert_eq!((pending.total, pending.items.len()), (4, 4));
        assert_eq!(pending.items.iter().map(|i| i.extra.created_at).collect::<Vec<_>>(), [100, 300, 400, 50]);
        let approved = list(&conn, |q| q.status = Some("approved".into()));
        assert_eq!(who(&approved), [("نور الهدى".to_string(), "s2".to_string())]);
        let rejected = list(&conn, |q| q.status = Some("rejected".into()));
        assert_eq!((rejected.total, rejected.items[0].extra.reason.as_deref()), (1, Some("سبب")));
        assert_eq!(list(&conn, |q| q.status = Some("  ".into())).total, 6, "blank = every status");
        let e = admin_teaching(&conn, &AdminTeachingQuery { status: Some("archived".into()), ..Default::default() }).unwrap_err();
        assert_eq!((e.0, e.1.contains("invalid_status")), (StatusCode::BAD_REQUEST, true));

        // search: teacher name (any case), e-mail, subject name in either language
        assert_eq!(list(&conn, |q| q.q = Some("SAMI".into())).total, 2);
        assert_eq!(who(&list(&conn, |q| q.q = Some("noor@school".into())))[0].0, "نور الهدى");
        assert_eq!(list(&conn, |q| q.q = Some("برمج".into())).total, 3, "subject name, Arabic");
        assert_eq!(list(&conn, |q| q.q = Some("programming".into())).total, 3, "subject name, English");
        assert_eq!(who(&list(&conn, |q| q.q = Some("ثقافة".into()))).len(), 2);
        assert_eq!(list(&conn, |q| q.q = Some("zzz".into())).total, 0);
        // wildcards in the search text are literal
        assert_eq!(who(&list(&conn, |q| q.q = Some("100%".into()))), [("100% Teacher".to_string(), "s2".to_string())]);
        assert_eq!(list(&conn, |q| q.q = Some("%".into())).total, 1, "a lone % matches only the name that contains one");
        assert_eq!(list(&conn, |q| q.q = Some("_".into())).total, 0);
        // search and tab combine
        assert_eq!(list(&conn, |q| {
            q.q = Some("sami".into());
            q.status = Some("rejected".into());
        })
        .total, 1);
        let e = admin_teaching(&conn, &AdminTeachingQuery { q: Some("x".repeat(101)), ..Default::default() }).unwrap_err();
        assert_eq!((e.0, e.1.contains("invalid_query")), (StatusCode::BAD_REQUEST, true));

        // paging: the total ignores limit/offset; the order is stable across pages
        let first = list(&conn, |q| q.limit = Some(2));
        let second = list(&conn, |q| {
            q.limit = Some(2);
            q.offset = Some(2);
        });
        assert_eq!((first.items.len(), first.total, second.items.len(), second.total), (2, 6, 2, 6));
        assert_eq!(first.items.iter().chain(second.items.iter()).map(|i| i.extra.created_at).collect::<Vec<_>>(), [100, 150, 200, 300]);
        let past = list(&conn, |q| q.offset = Some(99));
        assert_eq!((past.items.len(), past.total), (0, 6));
        assert_eq!(list(&conn, |q| q.limit = Some(0)).items.len(), 1, "limit is at least 1");
        assert_eq!(list(&conn, |q| q.offset = Some(-3)).items.len(), 6, "a negative offset is 0");

        // the row: everything the page already used, plus the new fields and the account state
        let item = serde_json::to_value(&approved.items[0]).unwrap();
        let expected: std::collections::BTreeSet<String> = [
            "teacher_id", "teacher_name", "subject_id", "subject_name", "institution_id", "institution_name", "status", "teacher_status",
            "created_at", "decided_at", "reason", "unit_path", "subject_active", "institution_active",
        ]
        .iter()
        .map(|k| k.to_string())
        .collect();
        assert_eq!(keys(&item), expected);
        assert_eq!(item["teacher_id"], users[0].id.as_str());
        let with_path = list(&conn, |q| q.q = Some("programming".into())).items.into_iter().find(|i| i.base.subject_id == "s1").unwrap();
        assert_eq!(with_path.extra.unit_path, ["كلية الحاسوب", "المستوى الأول"]);
        let json = serde_json::to_value(&all).unwrap();
        assert!(json["items"].is_array() && json["total"] == 6, "the response is {{items,total}}");

        // the default page size is 50, the maximum 200
        for i in 0..70 {
            conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES (?1,'i1','مادة',1,0)", params![format!("x{i}")]).unwrap();
            conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,?2,'pending',?3)", params![users[0].id, format!("x{i}"), 1000 + i]).unwrap();
        }
        let big = list(&conn, |_| {});
        assert_eq!((big.items.len(), big.total), (50, 76));
        assert_eq!(list(&conn, |q| q.limit = Some(1000)).items.len(), 76, "capped at 200, which is more than there is");
    }

    #[test]
    fn unit_paths_are_read_root_first_and_a_missing_unit_is_an_empty_path() {
        let conn = campus();
        assert_eq!(unit_path(&conn, Some("l1".into())).unwrap(), ["كلية الحاسوب", "المستوى الأول"]);
        assert_eq!(unit_path(&conn, Some("d1".into())).unwrap(), ["كلية الحاسوب"]);
        assert!(unit_path(&conn, None).unwrap().is_empty());
        assert!(unit_path(&conn, Some("gone".into())).unwrap().is_empty());
    }
}
