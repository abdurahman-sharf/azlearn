//! The student's learning screens (phase 4-2): the learning hub (`GET /my/learning`), a live session's page for a student
//! (`GET /live/{id}`, answered by `platform_content::get_live_handler`), the schedule (`GET /my/schedule`) and its calendar
//! file (`GET /my/schedule.ics`).
//!
//! Everything is scoped by the authenticated student's id (read from the session, never from the request) and computed on
//! demand: there is no shared cache. **Join links and lesson video links are for the enrolled** — every live session that
//! leaves this module goes through `platform_content::can_see_links`; the schedule JSON never carries a join link at all, and
//! the calendar file carries one only for a session the student may open.

use crate::platform::{bad, db_err, fold_like, lock, rate_limit, require_role, Res, User};
use crate::platform_content::{can_see_links, visible};
use crate::platform_exams::round2;
use crate::platform_student::{student_exams, ExamState};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::sync::Arc;

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;

// ───────── the learning hub ─────────

/// How far a student is in a course. The wire values are the snake_case names.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningState {
    NotStarted,
    InProgress,
    Completed,
}

impl LearningState {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "not_started" => Self::NotStarted,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            _ => return None,
        })
    }

    /// Position in the `all` list: what the student is in the middle of first, then what waits, then what is done.
    fn rank(self) -> u8 {
        match self {
            Self::InProgress => 0,
            Self::NotStarted => 1,
            Self::Completed => 2,
        }
    }

    fn of(completed: i64, total: i64) -> Self {
        if completed <= 0 {
            Self::NotStarted
        } else if completed >= total {
            Self::Completed
        } else {
            Self::InProgress
        }
    }
}

/// One course of the student's learning list.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LearningItem {
    pub course_id: String,
    pub title: String,
    /// The first 160 characters of the description.
    pub description: Option<String>,
    pub subject_id: String,
    pub subject_name: String,
    pub teacher_id: String,
    pub teacher_name: String,
    pub total: i64,
    pub completed: i64,
    /// Completed lessons in percent of the course's lessons, two decimals.
    pub percent: f64,
    pub state: LearningState,
    /// The first lesson (by position) the student has not completed; `null` for a finished course.
    pub next_lesson_id: Option<String>,
    /// When the student last completed a lesson of the course.
    pub last_at: Option<i64>,
    pub updated_at: i64,
}

/// **The one place that decides which courses a student can learn from and how far they are** (the learning hub, the home
/// overview's "continue learning" and `GET /my/progress` all read it): published courses of a teacher in good standing
/// (`platform_content::visible`: subject and institution active, assignment approved) in a subject the student is enrolled
/// in, with at least one lesson. `subject_id` and `like` (a [`fold_like`] pattern over the title and the description) narrow
/// it. Unordered apart from the course id; callers sort.
pub(crate) fn learning_items(conn: &Connection, student_id: &str, subject_id: Option<&str>, like: Option<&str>) -> Res<Vec<LearningItem>> {
    conn.prepare(&format!(
        "SELECT c.id, c.title, substr(c.description, 1, 160), c.subject_id, s.name_ar, c.teacher_id, u.full_name, c.updated_at,
                (SELECT count(*) FROM lessons l WHERE l.course_id = c.id),
                (SELECT count(*) FROM lesson_progress p JOIN lessons l ON l.id = p.lesson_id WHERE l.course_id = c.id AND p.student_id = ?1),
                (SELECT max(p.completed_at) FROM lesson_progress p JOIN lessons l ON l.id = p.lesson_id WHERE l.course_id = c.id AND p.student_id = ?1),
                (SELECT l.id FROM lessons l WHERE l.course_id = c.id
                   AND NOT EXISTS(SELECT 1 FROM lesson_progress p WHERE p.lesson_id = l.id AND p.student_id = ?1)
                 ORDER BY l.position, l.created_at, l.id LIMIT 1)
         FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id
         WHERE {}
           AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = c.subject_id)
           AND EXISTS(SELECT 1 FROM lessons l WHERE l.course_id = c.id)
           AND (?2 IS NULL OR c.subject_id = ?2)
           AND (?3 IS NULL OR ar_fold(c.title) LIKE ?3 ESCAPE '\\' OR ar_fold(COALESCE(c.description, '')) LIKE ?3 ESCAPE '\\')
         ORDER BY c.id",
        visible("c", "published")
    ))
    .map_err(db_err)?
    .query_map(params![student_id, subject_id, like], |r| {
        let (total, completed): (i64, i64) = (r.get(8)?, r.get(9)?);
        Ok(LearningItem {
            course_id: r.get(0)?,
            title: r.get(1)?,
            description: r.get(2)?,
            subject_id: r.get(3)?,
            subject_name: r.get(4)?,
            teacher_id: r.get(5)?,
            teacher_name: r.get(6)?,
            total,
            completed,
            percent: if total > 0 { round2(completed as f64 / total as f64 * 100.0) } else { 0.0 },
            state: LearningState::of(completed, total),
            next_lesson_id: r.get(11)?,
            last_at: r.get(10)?,
            updated_at: r.get(7)?,
        })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

/// `None` sorts last, whichever the direction; newest first for `Some`.
fn newest_first(a: Option<i64>, b: Option<i64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The list order: in progress (latest activity first), then not started (most recently updated first), then completed
/// (latest activity first); the course id breaks every tie, so a page boundary is stable.
fn learning_order(a: &LearningItem, b: &LearningItem) -> Ordering {
    let within = match a.state {
        LearningState::InProgress | LearningState::Completed => newest_first(a.last_at, b.last_at),
        LearningState::NotStarted => b.updated_at.cmp(&a.updated_at),
    };
    a.state.rank().cmp(&b.state.rank()).then(within).then_with(|| a.course_id.cmp(&b.course_id))
}

/// `GET /my/learning` query. Blank values count as absent.
#[derive(Deserialize, Default, Debug)]
pub struct LearningQuery {
    /// `all` (default) | `in_progress` | `not_started` | `completed`.
    pub state: Option<String>,
    pub subject_id: Option<String>,
    /// Title / description search (Arabic-folded, case-insensitive; `%`, `_` and `\` are literal).
    pub q: Option<String>,
    /// Default 24, at most 60.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// How many courses are in each state (after the subject / search filters, whatever the chosen state).
#[derive(Serialize, Debug, Default, PartialEq, Clone)]
pub struct LearningCounts {
    pub in_progress: i64,
    pub not_started: i64,
    pub completed: i64,
}

#[derive(Serialize, Debug)]
pub struct LearningPage {
    pub items: Vec<LearningItem>,
    /// Courses of the chosen state, before `limit` / `offset`.
    pub total: i64,
    pub counts: LearningCounts,
}

const LEARNING_DEFAULT_LIMIT: i64 = 24;
const LEARNING_MAX_LIMIT: i64 = 60;
const MAX_SEARCH_CHARS: usize = 100;

fn blank(o: &Option<String>) -> Option<String> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub(crate) fn learning_page(conn: &Connection, student: &User, q: &LearningQuery) -> Res<LearningPage> {
    let state = match blank(&q.state).as_deref() {
        None | Some("all") => None,
        Some(s) => Some(LearningState::parse(s).ok_or_else(|| bad("invalid_filter"))?),
    };
    let like = blank(&q.q).map(|s| fold_like(&s.chars().take(MAX_SEARCH_CHARS).collect::<String>()));
    let mut items = learning_items(conn, &student.id, blank(&q.subject_id).as_deref(), like.as_deref())?;
    let mut counts = LearningCounts::default();
    for i in &items {
        match i.state {
            LearningState::InProgress => counts.in_progress += 1,
            LearningState::NotStarted => counts.not_started += 1,
            LearningState::Completed => counts.completed += 1,
        }
    }
    items.retain(|i| state.map_or(true, |s| i.state == s));
    items.sort_by(learning_order);
    let total = items.len() as i64;
    let limit = q.limit.unwrap_or(LEARNING_DEFAULT_LIMIT).clamp(1, LEARNING_MAX_LIMIT) as usize;
    let offset = q.offset.unwrap_or(0).max(0) as usize;
    Ok(LearningPage { items: items.into_iter().skip(offset).take(limit).collect(), total, counts })
}

pub async fn my_learning_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<LearningQuery>) -> Res<Json<LearningPage>> {
    let student = require_role(&s, &h, "student")?;
    learning_page(&*lock(&s)?, &student, &q).map(Json)
}

// ───────── a live session's page ─────────

/// Where a live session is in time. The wire values are the snake_case names.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LivePhase {
    Upcoming,
    LiveNow,
    Ended,
}

/// `upcoming` before the start, `live_now` from the start until the end (the end itself is over), `ended` after.
pub(crate) fn live_phase(starts_at: i64, duration_min: i64, now: i64) -> LivePhase {
    if now < starts_at {
        LivePhase::Upcoming
    } else if now < starts_at + duration_min * MINUTE_MS {
        LivePhase::LiveNow
    } else {
        LivePhase::Ended
    }
}

/// A live session as a student's page shows it. `join_url` is `""` (and `locked` true) unless the student is enrolled in the
/// subject — see `platform_content::can_see_links`.
#[derive(Serialize, Debug, PartialEq)]
pub struct StudentLive {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub teacher_id: String,
    pub teacher_name: String,
    pub subject_id: String,
    pub subject_name: String,
    pub starts_at: i64,
    pub duration_min: i64,
    /// Always `scheduled`: a draft / cancelled / hidden session is a 404, never a page.
    pub status: String,
    pub join_url: String,
    pub locked: bool,
    /// The student is enrolled in the session's subject.
    pub enrolled: bool,
    pub phase: LivePhase,
    /// The join button opens from here: 15 minutes before the start.
    pub joinable_from: i64,
}

/// The page of ONE session for a student — only a session that is scheduled and shown to students (`visible`: teacher in good
/// standing, subject and institution active); anything else is the same 404 whether it never existed or is hidden.
pub(crate) fn student_live(conn: &Connection, student: &User, id: &str, now: i64) -> Res<StudentLive> {
    let (mut live, owner): (StudentLive, String) = conn
        .query_row(
            &format!(
                "SELECT l.id, l.title, l.description, l.teacher_id, u.full_name, l.subject_id, s.name_ar, l.starts_at, l.duration_min, l.join_url, l.status
                 FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id
                 WHERE l.id = ?1 AND {}",
                visible("l", "scheduled")
            ),
            params![id],
            |r| {
                let (starts_at, duration_min): (i64, i64) = (r.get(7)?, r.get(8)?);
                Ok((
                    StudentLive {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        description: r.get(2)?,
                        teacher_id: r.get(3)?,
                        teacher_name: r.get(4)?,
                        subject_id: r.get(5)?,
                        subject_name: r.get(6)?,
                        starts_at,
                        duration_min,
                        status: r.get(10)?,
                        join_url: r.get(9)?,
                        locked: false,
                        enrolled: false,
                        phase: live_phase(starts_at, duration_min, now),
                        joinable_from: starts_at - crate::platform_reminders::LIVE_LEAD_MS,
                    },
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    live.enrolled = crate::platform_exams::is_enrolled(conn, &student.id, &live.subject_id)?;
    if !can_see_links(conn, student, &owner, &live.subject_id)? {
        live.locked = true;
        live.join_url = String::new();
    }
    Ok(live)
}

// ───────── the schedule ─────────

/// What a schedule row is. The wire values are the snake_case names.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    Live,
    ExamOpens,
    ExamCloses,
}

impl ScheduleKind {
    fn rank(self) -> u8 {
        match self {
            Self::Live => 0,
            Self::ExamOpens => 1,
            Self::ExamCloses => 2,
        }
    }
}

/// One row of the schedule. **No join link**: the page of the session is where the link is (and only for the enrolled).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ScheduleItem {
    pub kind: ScheduleKind,
    /// The live session's or the exam's id.
    pub id: String,
    pub title: String,
    pub subject_id: String,
    pub subject_name: String,
    pub teacher_name: String,
    pub at: i64,
    /// A live session's end (`at` + duration); `null` for an exam.
    pub ends_at: Option<i64>,
    /// A live session's length in minutes; `null` for an exam.
    pub duration_min: Option<i64>,
    /// A live session whose join link is withheld from this student (not enrolled); `false` for an exam.
    pub locked: bool,
    pub link: String,
}

/// A schedule row together with what only the calendar file may carry: the join link of a session the student may open
/// (`None` when it is withheld, and for exams). Kept apart from [`ScheduleItem`] so the JSON of the schedule can never hold it.
#[derive(Clone, Debug)]
pub(crate) struct ScheduleEntry {
    pub item: ScheduleItem,
    pub join_url: Option<String>,
}

pub(crate) const SCHEDULE_LIMIT: usize = 300;
const SCHEDULE_DEFAULT_BEFORE_MS: i64 = 2 * 60 * MINUTE_MS;
const SCHEDULE_DEFAULT_SPAN_MS: i64 = 7 * DAY_MS;
const SCHEDULE_MAX_SPAN_MS: i64 = 62 * DAY_MS;

/// `GET /my/schedule` query: both optional, milliseconds since the epoch.
#[derive(Deserialize, Default, Debug)]
pub struct ScheduleQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// The window of a schedule request. Without `from` the window starts two hours ago (a session that is on right now, or
/// just ended, is still on the list); without `to` it is a week long. 400 `invalid_range` for text that is not a time,
/// `from >= to`, or a window longer than 62 days.
pub(crate) fn schedule_window(q: &ScheduleQuery, now: i64) -> Res<(i64, i64)> {
    let parse = |o: &Option<String>| -> Res<Option<i64>> {
        match blank(o) {
            None => Ok(None),
            Some(v) => v.parse::<i64>().ok().filter(|n| *n >= 0).map(Some).ok_or_else(|| bad("invalid_range")),
        }
    };
    let from = parse(&q.from)?.unwrap_or(now - SCHEDULE_DEFAULT_BEFORE_MS);
    let to = parse(&q.to)?.unwrap_or_else(|| from.saturating_add(SCHEDULE_DEFAULT_SPAN_MS));
    if from >= to || to - from > SCHEDULE_MAX_SPAN_MS {
        return Err(bad("invalid_range"));
    }
    Ok((from, to))
}

/// The student's schedule in `[from, to]` (closed on both ends), soonest first, at most `cap` rows:
///
/// * **live sessions** that are scheduled and shown to students (`visible`), in an enrolled subject or by a followed teacher,
///   and that overlap the window (one that ended before `from` is out; one that starts after `to` is out);
/// * **exam openings**: a published exam of an enrolled subject, shown to students, whose opening time is in the window and
///   still ahead;
/// * **exam closings**: the same exams, whose closing time is in the window, while the student can still act on them
///   (`open`, `upcoming` or `in_progress` in `platform_student::student_view`).
pub(crate) fn schedule_entries(conn: &Connection, student: &User, from: i64, to: i64, now: i64, cap: usize) -> Res<Vec<ScheduleEntry>> {
    let mut out: Vec<ScheduleEntry> = Vec::new();
    type Row = (String, String, String, String, String, String, i64, i64, String);
    let lives: Vec<Row> = conn
        .prepare(&format!(
            "SELECT l.id, l.title, l.subject_id, s.name_ar, u.full_name, l.teacher_id, l.starts_at, l.duration_min, l.join_url
             FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id
             WHERE {}
               AND (l.subject_id IN (SELECT subject_id FROM subject_enrollments WHERE student_id = ?1)
                    OR l.teacher_id IN (SELECT teacher_id FROM teacher_follows WHERE student_id = ?1))
               AND l.starts_at <= ?3 AND l.starts_at + l.duration_min * 60000 >= ?2
             ORDER BY l.starts_at, l.id LIMIT ?4",
            visible("l", "scheduled")
        ))
        .map_err(db_err)?
        .query_map(params![student.id, from, to, cap as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    for (id, title, subject_id, subject_name, teacher_name, owner, starts_at, duration_min, join_url) in lives {
        let allowed = can_see_links(conn, student, &owner, &subject_id)?;
        out.push(ScheduleEntry {
            join_url: allowed.then_some(join_url),
            item: ScheduleItem {
                kind: ScheduleKind::Live,
                link: format!("/platform/live/{id}"),
                id,
                title,
                subject_id,
                subject_name,
                teacher_name,
                at: starts_at,
                ends_at: Some(starts_at + duration_min * MINUTE_MS),
                duration_min: Some(duration_min),
                locked: !allowed,
            },
        });
    }
    // exams: the very rows (and states) of "My exams"; only the ones the student can still reach
    for e in student_exams(conn, &student.id, None, None, now)? {
        if !e.visible || e.phase != "published" {
            continue;
        }
        let mut push = |kind: ScheduleKind, at: i64| {
            out.push(ScheduleEntry {
                join_url: None,
                item: ScheduleItem {
                    kind,
                    link: format!("/platform/assessments/{}", e.id),
                    id: e.id.clone(),
                    title: e.title.clone(),
                    subject_id: e.subject_id.clone(),
                    subject_name: e.subject_name.clone(),
                    teacher_name: e.teacher_name.clone(),
                    at,
                    ends_at: None,
                    duration_min: None,
                    locked: false,
                },
            });
        };
        if let Some(o) = e.opens_at.filter(|o| *o >= from && *o <= to && *o > now) {
            push(ScheduleKind::ExamOpens, o);
        }
        if let Some(c) = e.closes_at.filter(|c| *c >= from && *c <= to) {
            if matches!(e.state, ExamState::Open | ExamState::Upcoming | ExamState::InProgress) {
                push(ScheduleKind::ExamCloses, c);
            }
        }
    }
    out.sort_by(|a, b| a.item.at.cmp(&b.item.at).then_with(|| a.item.id.cmp(&b.item.id)).then_with(|| a.item.kind.rank().cmp(&b.item.kind.rank())));
    out.truncate(cap);
    Ok(out)
}

#[derive(Serialize, Debug)]
pub struct SchedulePage {
    pub items: Vec<ScheduleItem>,
}

pub async fn my_schedule_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<ScheduleQuery>) -> Res<Json<SchedulePage>> {
    let student = require_role(&s, &h, "student")?;
    let now = now_ms();
    let (from, to) = schedule_window(&q, now)?;
    let entries = schedule_entries(&*lock(&s)?, &student, from, to, now, SCHEDULE_LIMIT)?;
    Ok(Json(SchedulePage { items: entries.into_iter().map(|e| e.item).collect() }))
}

// ───────── the calendar file (RFC 5545) ─────────

/// Language of the fixed words in the calendar file (the titles are the teachers' own text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IcsLang {
    Ar,
    En,
}

impl IcsLang {
    fn parse(s: Option<&str>) -> Self {
        if s.map(|s| s.trim().eq_ignore_ascii_case("en")).unwrap_or(false) {
            Self::En
        } else {
            Self::Ar
        }
    }
}

const ICS_BEFORE_MS: i64 = DAY_MS;
const ICS_AFTER_MS: i64 = 90 * DAY_MS;
const ICS_LIMIT: usize = 1000;
/// Exam events are blocks of this length: starting at the opening time, or ending at the closing time.
const ICS_EXAM_BLOCK_MS: i64 = 15 * MINUTE_MS;

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `YYYYMMDDTHHMMSSZ`, UTC.
pub(crate) fn ics_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// RFC 5545 TEXT: backslash, semicolon and comma are escaped, a newline becomes `\n`, every other control character
/// (carriage returns, tabs, NULs, ...) is dropped so a title can never inject a line of its own.
pub(crate) fn ics_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Appends `line` and its terminating CRLF, folded so no physical line is longer than 75 octets: a break is `CRLF` + one
/// space, the space counts towards the next line's 75, and a multi-byte character is never split.
pub(crate) fn ics_fold(line: &str, out: &mut String) {
    let mut width = 0usize;
    for c in line.chars() {
        let n = c.len_utf8();
        if width + n > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += n;
    }
    out.push_str("\r\n");
}

/// Only characters that are safe in a UID (ids are uuids; anything else is dropped rather than trusted).
fn ics_uid_part(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

/// The schedule as an iCalendar file (CRLF line ends, folded at 75 octets, UTC times). `now` stamps `DTSTAMP`. A live session
/// is an event with its start and end, a `VALARM` 15 minutes ahead, and its join link in the description **only** when
/// the entry carries one (the student may open it); an exam is a 15-minute block starting at its opening time (`exam-<id>-opens`)
/// or ending at its closing time (`exam-<id>-closes`). No e-mail address, no other student's data.
pub(crate) fn build_ics(entries: &[ScheduleEntry], now: i64, lang: IcsLang) -> String {
    let (opens, closes, teacher_l, subject_l, join_l) = match lang {
        IcsLang::Ar => ("فتح الامتحان", "إغلاق الامتحان", "المعلم", "المادة", "رابط الانضمام"),
        IcsLang::En => ("Exam opens", "Exam closes", "Teacher", "Subject", "Join link"),
    };
    let mut out = String::new();
    let mut line = |l: &str| ics_fold(l, &mut out);
    line("BEGIN:VCALENDAR");
    line("VERSION:2.0");
    line("PRODID:-//azlearn//schedule//AR");
    line("CALSCALE:GREGORIAN");
    line("METHOD:PUBLISH");
    line("X-WR-CALNAME:azlearn");
    for e in entries {
        let i = &e.item;
        let (uid, start, end, summary) = match i.kind {
            ScheduleKind::Live => (format!("live-{}@azlearn", ics_uid_part(&i.id)), i.at, i.ends_at.unwrap_or(i.at), i.title.clone()),
            ScheduleKind::ExamOpens => (format!("exam-{}-opens@azlearn", ics_uid_part(&i.id)), i.at, i.at + ICS_EXAM_BLOCK_MS, format!("{opens}: {}", i.title)),
            ScheduleKind::ExamCloses => (format!("exam-{}-closes@azlearn", ics_uid_part(&i.id)), i.at - ICS_EXAM_BLOCK_MS, i.at, format!("{closes}: {}", i.title)),
        };
        let mut description = format!("{teacher_l}: {}\n{subject_l}: {}", i.teacher_name, i.subject_name);
        if let Some(url) = e.join_url.as_deref().filter(|u| !u.is_empty()) {
            description.push_str(&format!("\n{join_l}: {url}"));
        }
        line("BEGIN:VEVENT");
        line(&format!("UID:{uid}"));
        line(&format!("DTSTAMP:{}", ics_utc(now)));
        line(&format!("DTSTART:{}", ics_utc(start)));
        line(&format!("DTEND:{}", ics_utc(end)));
        line(&format!("SUMMARY:{}", ics_escape(&summary)));
        line(&format!("DESCRIPTION:{}", ics_escape(&description)));
        if i.kind == ScheduleKind::Live {
            line("BEGIN:VALARM");
            line("ACTION:DISPLAY");
            line(&format!("DESCRIPTION:{}", ics_escape(&i.title)));
            line("TRIGGER:-PT15M");
            line("END:VALARM");
        }
        line("END:VEVENT");
    }
    line("END:VCALENDAR");
    out
}

/// `GET /my/schedule.ics`: the next 90 days (and yesterday) as a calendar file the student's own calendar app can import.
/// 10 downloads a minute per student. `?lang=en` writes the fixed words in English (Arabic by default).
#[derive(Deserialize, Default, Debug)]
pub struct IcsQuery {
    pub lang: Option<String>,
}

pub async fn schedule_ics_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<IcsQuery>) -> Res<Response> {
    let student = require_role(&s, &h, "student")?;
    let now = now_ms();
    let body = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("ics:{}", student.id), MINUTE_MS, 10)?;
        build_ics(&schedule_entries(&conn, &student, now - ICS_BEFORE_MS, now + ICS_AFTER_MS, now, ICS_LIMIT)?, now, IcsLang::parse(q.lang.as_deref()))
    };
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8")),
            (header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"azlearn-schedule.ics\"")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        body,
    )
        .into_response())
}

/// The student's learning routes (merged into the app in `main.rs`).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/platform/my/learning", get(my_learning_handler))
        .route("/api/platform/my/schedule", get(my_schedule_handler))
        .route("/api/platform/my/schedule.ics", get(schedule_ics_handler))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod http_tests;
