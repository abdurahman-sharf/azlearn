//! The student's own screens (phase 4-1): what every exam is *for me* ([`student_view`], the ONE rule), the "My exams"
//! list (`GET /my/exams`) and the home screen numbers (`GET /student/overview`).
//!
//! Everything is scoped by the authenticated student's id (read from the session, never from the request) and computed
//! on demand, like the teacher's dashboard (`platform_teacher`): there is no shared cache.
//!
//! **The student's exam scope** (one definition, used by the list and by the overview):
//! * (a) every exam students can see (`published` or `closed`, and `platform_content::exam_standing` holds) in a subject
//!   the student is enrolled in, plus
//! * (b) every exam on which the student has at least one attempt of any status, even when it is no longer visible, no
//!   longer in an enrolled subject, or archived — only so the student can reach their own results. Such a row has
//!   `visible: false` and never offers a start; a row never carries questions or answers.
//!
//! **States** ([`ExamState`], in this order of precedence): `in_progress` (an attempt is running: its deadline + grace is
//! still ahead — overdue attempts are settled first, `platform_exams::settle_student`), `open` (the student can start
//! one now), `upcoming` (published, not open yet), `done` (at least one submitted attempt), `missed` (everything else:
//! the exam closed or the attempts ran out without a submitted attempt, or the only thing in the way is something the
//! student is told in `reason`). `can_start` is exactly `state == open`; `reason` says why not (see [`Blocked`]).

use crate::platform::{bad, db_err, like_pattern, lock, require_role, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::platform_content::{exam_standing, visible};
use crate::platform_exams::{deadline, effective_phase, is_released, passed_flag, round2, settle_student, AssessmentInfo, GRACE_MS};
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

// ───────── the one rule: an exam's state for one student ─────────

/// What an exam is for the signed-in student. The wire values are the snake_case names.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExamState {
    InProgress,
    Open,
    Upcoming,
    Done,
    Missed,
}

impl ExamState {
    /// `tab` of `GET /my/exams` (and the wire names).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "in_progress" => Self::InProgress,
            "open" => Self::Open,
            "upcoming" => Self::Upcoming,
            "done" => Self::Done,
            "missed" => Self::Missed,
            _ => return None,
        })
    }

    /// Position in the `all` list (and the precedence of the states).
    fn rank(self) -> u8 {
        match self {
            Self::InProgress => 0,
            Self::Open => 1,
            Self::Upcoming => 2,
            Self::Done => 3,
            Self::Missed => 4,
        }
    }
}

/// Why the student cannot start the exam now (`None` when they can, or while an attempt is running). When several hold,
/// the first in this order is reported: what nobody can change first, what the student can do something about last.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// Closed (manually or by its closing time), archived, or no longer shown to students at all.
    Closed,
    /// Every allowed attempt has been used (expired and in-progress ones count, like `start_attempt`).
    NoAttempts,
    /// The student is not enrolled in the exam's subject (they can enrol from the exam page).
    NotEnrolled,
    /// It has not opened yet (`opens_at` is ahead).
    NotOpen,
}

/// What [`student_view`] needs to know about the exam.
#[derive(Clone, Debug)]
pub(crate) struct ExamFacts {
    /// The stored status (`published`, `closed`, ...), not the effective phase.
    pub status: String,
    pub opens_at: Option<i64>,
    pub closes_at: Option<i64>,
    pub max_attempts: i64,
    pub duration_min: Option<i64>,
    pub total_points: f64,
    pub pass_mark: Option<f64>,
    pub release_mode: String,
    /// Students can see it: `published` or `closed`, and `platform_content::exam_standing` holds.
    pub shown: bool,
    /// The student is enrolled in its subject.
    pub enrolled: bool,
}

impl ExamFacts {
    pub(crate) fn from_info(info: &AssessmentInfo, shown: bool, enrolled: bool) -> Self {
        ExamFacts {
            status: info.status.clone(),
            opens_at: info.opens_at,
            closes_at: info.closes_at,
            max_attempts: info.max_attempts,
            duration_min: info.duration_min,
            total_points: info.total_points,
            pass_mark: info.pass_mark,
            release_mode: info.release_mode.clone(),
            shown,
            enrolled,
        }
    }
}

/// One of the student's own attempts on the exam.
#[derive(Clone, Debug)]
pub(crate) struct AttemptFact {
    pub id: String,
    pub status: String,
    pub started_at: i64,
    pub submitted_at: Option<i64>,
    pub score: f64,
    pub pending: i64,
}

impl AttemptFact {
    /// When it was handed in (or started, for the rare row without a submission time).
    fn stamp(&self) -> i64 {
        self.submitted_at.unwrap_or(self.started_at)
    }
}

/// The attempt that is running right now, and when the server ends it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RunningAttempt {
    pub attempt_id: String,
    pub ends_at: i64,
}

/// The student's best submitted attempt (by score, newest on a tie). While the result is withheld (`released: false`,
/// release mode `after_close` before the exam closes) `score`, `percent`, `passed` and `pending` are `null` — and the
/// attempt chosen is simply the newest submitted one, so not even "which of my attempts did better" leaks.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BestAttempt {
    pub attempt_id: String,
    pub score: Option<f64>,
    pub total: f64,
    /// `score / total` in percent, two decimals (`null` when withheld or the total is 0).
    pub percent: Option<f64>,
    /// `platform_exams::passed_flag`: `null` without a pass mark or while written answers wait for a grade.
    pub passed: Option<bool>,
    pub released: bool,
    /// Written answers of this attempt still waiting for a grade (`null` when withheld).
    pub pending: Option<i64>,
}

/// The verdict of [`student_view`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudentView {
    pub state: ExamState,
    pub can_start: bool,
    pub reason: Option<Blocked>,
    /// The effective phase (`platform_exams::effective_phase`).
    pub phase: String,
    /// All of the student's attempts, any status (what `start_attempt` compares with `max_attempts`).
    pub attempts_used: i64,
    pub in_progress: Option<RunningAttempt>,
    pub best: Option<BestAttempt>,
    /// The student's newest attempt of any status.
    pub last_attempt_id: Option<String>,
    /// When the newest submitted attempt was handed in (orders the `done` tab).
    pub last_submitted_at: Option<i64>,
}

/// **The one function that decides what an exam is for a student** (`GET /my/exams`, the exam page, the overview and the
/// "available" list all call it). `attempts` must be this student's attempts on this exam only, with overdue ones
/// already settled (an in-progress attempt past its deadline + grace is not counted as running either way).
pub(crate) fn student_view(exam: &ExamFacts, attempts: &[AttemptFact], now: i64) -> StudentView {
    let phase = effective_phase(&exam.status, exam.closes_at, now);
    let used = attempts.len() as i64;
    let running = attempts
        .iter()
        .filter(|t| t.status == "in_progress")
        .map(|t| (t, deadline(t.started_at, exam.duration_min)))
        .filter(|(_, ends)| now <= ends + GRACE_MS)
        .max_by(|(a, _), (b, _)| a.started_at.cmp(&b.started_at).then_with(|| a.id.cmp(&b.id)))
        .map(|(t, ends)| RunningAttempt { attempt_id: t.id.clone(), ends_at: ends });
    let submitted: Vec<&AttemptFact> = attempts.iter().filter(|t| t.status == "submitted").collect();

    // published, not past its closing time, and shown to students (subject, institution, teacher in good standing)
    let live = exam.status == "published" && phase == "published" && exam.shown;
    let opened = exam.opens_at.map_or(true, |o| o <= now);
    let attempts_left = used < exam.max_attempts;
    let state = if running.is_some() {
        ExamState::InProgress
    } else if live && opened && attempts_left && exam.enrolled {
        ExamState::Open
    } else if live && !opened {
        ExamState::Upcoming
    } else if !submitted.is_empty() {
        ExamState::Done
    } else {
        ExamState::Missed
    };
    let reason = match state {
        ExamState::InProgress | ExamState::Open => None,
        _ => Some(if !live {
            Blocked::Closed
        } else if !attempts_left {
            Blocked::NoAttempts
        } else if !exam.enrolled {
            Blocked::NotEnrolled
        } else {
            // live, attempts left and enrolled: only the opening time can stand in the way
            Blocked::NotOpen
        }),
    };

    let released = is_released(&exam.release_mode, &exam.status, exam.closes_at, now);
    let by_time = |a: &&&AttemptFact, b: &&&AttemptFact| a.stamp().cmp(&b.stamp()).then_with(|| a.id.cmp(&b.id));
    let pick = if released {
        submitted.iter().max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal).then_with(|| by_time(a, b)))
    } else {
        submitted.iter().max_by(by_time)
    };
    let total = exam.total_points;
    let best = pick.map(|t| BestAttempt {
        attempt_id: t.id.clone(),
        score: released.then_some(t.score),
        total,
        percent: (released && total > 0.0).then(|| round2(t.score / total * 100.0)),
        passed: if released { passed_flag(t.score, total, t.pending, exam.pass_mark) } else { None },
        released,
        pending: released.then_some(t.pending),
    });
    let last_attempt_id = attempts.iter().max_by(|a, b| a.started_at.cmp(&b.started_at).then_with(|| a.id.cmp(&b.id))).map(|t| t.id.clone());
    StudentView {
        can_start: state == ExamState::Open,
        state,
        reason,
        phase,
        attempts_used: used,
        in_progress: running,
        best,
        last_attempt_id,
        last_submitted_at: submitted.iter().map(|t| t.stamp()).max(),
    }
}

// ───────── reading the student's data ─────────

/// This student's attempts, grouped by exam (ONE query; `assessment_id` narrows it to one exam).
pub(crate) fn attempt_facts(conn: &Connection, student_id: &str, assessment_id: Option<&str>) -> Res<HashMap<String, Vec<AttemptFact>>> {
    let mut out: HashMap<String, Vec<AttemptFact>> = HashMap::new();
    let rows = conn
        .prepare(
            "SELECT assessment_id, id, status, started_at, submitted_at, score, pending FROM attempts
             WHERE student_id = ?1 AND (?2 IS NULL OR assessment_id = ?2)",
        )
        .map_err(db_err)?
        .query_map(params![student_id, assessment_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                AttemptFact { id: r.get(1)?, status: r.get(2)?, started_at: r.get(3)?, submitted_at: r.get(4)?, score: r.get(5)?, pending: r.get(6)? },
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    for (exam, fact) in rows {
        out.entry(exam).or_default().push(fact);
    }
    Ok(out)
}

/// SQL column: students can see exam `a` (`published`/`closed` and in good standing); 0/1, never NULL.
fn shown_sql() -> String {
    format!("COALESCE(a.status IN ('published','closed') AND {}, 0)", exam_standing("a"))
}

/// The state of ONE exam for one student (the exam page). The caller settles the student's overdue attempts on it first
/// (the page does, with `expire_stale`).
pub(crate) fn exam_view(conn: &Connection, student_id: &str, info: &AssessmentInfo, now: i64) -> Res<StudentView> {
    let shown: bool = conn
        .query_row(
            &format!("SELECT {} FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id WHERE a.id = ?1", shown_sql()),
            params![info.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?
        .unwrap_or(false);
    let enrolled = crate::platform_exams::is_enrolled(conn, student_id, &info.subject_id)?;
    let attempts = attempt_facts(conn, student_id, Some(&info.id))?.remove(&info.id).unwrap_or_default();
    Ok(student_view(&ExamFacts::from_info(info, shown, enrolled), &attempts, now))
}

/// One exam of the student's list (`GET /my/exams`). Never carries questions or answers.
#[derive(Serialize, Debug, Clone)]
pub struct StudentExamRow {
    pub id: String,
    pub title: String,
    pub subject_id: String,
    pub subject_name: String,
    pub teacher_name: String,
    pub question_count: i64,
    pub total_points: f64,
    pub duration_min: Option<i64>,
    pub opens_at: Option<i64>,
    pub closes_at: Option<i64>,
    pub max_attempts: i64,
    pub pass_mark: Option<f64>,
    pub release_mode: String,
    pub phase: String,
    pub state: ExamState,
    pub can_start: bool,
    pub reason: Option<Blocked>,
    pub attempts_used: i64,
    pub in_progress_attempt: Option<RunningAttempt>,
    pub best: Option<BestAttempt>,
    pub last_attempt_id: Option<String>,
    /// In scope (a): students can see it and the student is enrolled in its subject. `false` = a row the student only
    /// keeps because of their own attempts (hidden subject or institution, archived, no longer enrolled): it leads to
    /// their results, never to a start.
    pub visible: bool,
    #[serde(skip)]
    last_submitted_at: Option<i64>,
}

/// Search text longer than this is cut (a title is at most 200 characters; nobody types more to find one).
const MAX_SEARCH_CHARS: usize = 100;

/// Every exam of the student's scope (see the module docs), filtered by subject and title search, with its state, in the
/// list order. Two reads: the exams (one query) and the student's attempts (one query); overdue attempts are settled first.
/// The first condition of the query (my enrolled subjects, or exams with my attempts) only narrows the rows so SQLite can
/// answer from the indexes (a multi-index OR) instead of reading every exam; the second one is the scope itself.
pub(crate) fn student_exams(conn: &Connection, student_id: &str, subject_id: Option<&str>, search: Option<&str>, now: i64) -> Res<Vec<StudentExamRow>> {
    settle_student(conn, student_id, now)?;
    let like = search.map(like_pattern);
    let standing = exam_standing("a");
    let sql = format!(
        "SELECT a.id, a.title, a.subject_id, s.name_ar, COALESCE(u.full_name, cb.full_name, ''), a.question_count, a.total_points,
                a.duration_min, a.opens_at, a.closes_at, a.max_attempts, a.pass_mark, a.release_mode, a.status, {shown},
                EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = a.subject_id)
         FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id LEFT JOIN users cb ON cb.id = a.created_by JOIN subjects s ON s.id = a.subject_id
         WHERE (a.subject_id IN (SELECT e.subject_id FROM subject_enrollments e WHERE e.student_id = ?1)
                OR a.id IN (SELECT t.assessment_id FROM attempts t WHERE t.student_id = ?1))
           AND ((a.status IN ('published','closed') AND {standing}
                 AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = a.subject_id))
                OR EXISTS(SELECT 1 FROM attempts t WHERE t.assessment_id = a.id AND t.student_id = ?1))
           AND (?2 IS NULL OR a.subject_id = ?2)
           AND (?3 IS NULL OR lower(a.title) LIKE ?3 ESCAPE '\\')",
        shown = shown_sql()
    );
    let exams = conn
        .prepare(&sql)
        .map_err(db_err)?
        .query_map(params![student_id, subject_id, like], |r| {
            let facts = ExamFacts {
                total_points: r.get(6)?,
                duration_min: r.get(7)?,
                opens_at: r.get(8)?,
                closes_at: r.get(9)?,
                max_attempts: r.get(10)?,
                pass_mark: r.get(11)?,
                release_mode: r.get(12)?,
                status: r.get(13)?,
                shown: r.get(14)?,
                enrolled: r.get(15)?,
            };
            let head: (String, String, String, String, String, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            Ok((head, facts))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let mut attempts = attempt_facts(conn, student_id, None)?;
    let mut rows: Vec<StudentExamRow> = exams
        .into_iter()
        .map(|((id, title, subject_id, subject_name, teacher_name, question_count), facts)| {
            let mine = attempts.remove(&id).unwrap_or_default();
            let v = student_view(&facts, &mine, now);
            StudentExamRow {
                visible: facts.shown && facts.enrolled,
                id,
                title,
                subject_id,
                subject_name,
                teacher_name,
                question_count,
                total_points: facts.total_points,
                duration_min: facts.duration_min,
                opens_at: facts.opens_at,
                closes_at: facts.closes_at,
                max_attempts: facts.max_attempts,
                pass_mark: facts.pass_mark,
                release_mode: facts.release_mode,
                phase: v.phase,
                state: v.state,
                can_start: v.can_start,
                reason: v.reason,
                attempts_used: v.attempts_used,
                in_progress_attempt: v.in_progress,
                best: v.best,
                last_attempt_id: v.last_attempt_id,
                last_submitted_at: v.last_submitted_at,
            }
        })
        .collect();
    rows.sort_by(list_order);
    Ok(rows)
}

/// `None` sorts after every value, whichever the direction.
fn asc_nulls_last(a: Option<i64>, b: Option<i64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn desc_nulls_last(a: Option<i64>, b: Option<i64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        _ => asc_nulls_last(a, b),
    }
}

/// The list order: by state (in progress, open, upcoming, done, missed), then within a state the order that makes sense
/// for it — the running attempt that ends first; open ones closing soonest (no closing time last); upcoming ones opening
/// soonest; done ones most recently handed in; missed ones most recently closed. Title and id break ties, so a page
/// boundary is stable.
fn list_order(a: &StudentExamRow, b: &StudentExamRow) -> Ordering {
    let within = match a.state {
        ExamState::InProgress => asc_nulls_last(a.in_progress_attempt.as_ref().map(|r| r.ends_at), b.in_progress_attempt.as_ref().map(|r| r.ends_at)),
        ExamState::Open => asc_nulls_last(a.closes_at, b.closes_at),
        ExamState::Upcoming => asc_nulls_last(a.opens_at, b.opens_at),
        ExamState::Done => desc_nulls_last(a.last_submitted_at, b.last_submitted_at),
        ExamState::Missed => desc_nulls_last(a.closes_at, b.closes_at),
    };
    a.state.rank().cmp(&b.state.rank()).then(within).then_with(|| a.title.cmp(&b.title)).then_with(|| a.id.cmp(&b.id))
}

// ───────── GET /my/exams ─────────

/// `GET /my/exams` query. Blank values count as absent.
#[derive(Deserialize, Default, Debug)]
pub struct MyExamsQuery {
    /// `all` (default) | `open` | `upcoming` | `in_progress` | `done` | `missed`.
    pub tab: Option<String>,
    pub subject_id: Option<String>,
    /// Title search (case-insensitive, `%` and `_` are literal).
    pub q: Option<String>,
    /// Default 25, at most 100.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// How many exams of the scope are in each state (after the subject / search filters, whatever the tab).
#[derive(Serialize, Debug, Default, PartialEq, Clone)]
pub struct StateCounts {
    pub open: i64,
    pub upcoming: i64,
    pub in_progress: i64,
    pub done: i64,
    pub missed: i64,
}

impl StateCounts {
    fn of(rows: &[StudentExamRow]) -> Self {
        let mut c = StateCounts::default();
        for r in rows {
            *match r.state {
                ExamState::Open => &mut c.open,
                ExamState::Upcoming => &mut c.upcoming,
                ExamState::InProgress => &mut c.in_progress,
                ExamState::Done => &mut c.done,
                ExamState::Missed => &mut c.missed,
            } += 1;
        }
        c
    }
}

#[derive(Serialize, Debug)]
pub struct MyExamsPage {
    pub items: Vec<StudentExamRow>,
    /// Rows of the chosen tab, before `limit` / `offset`.
    pub total: i64,
    pub counts: StateCounts,
}

pub(crate) const DEFAULT_LIMIT: i64 = 25;
pub(crate) const MAX_LIMIT: i64 = 100;

pub(crate) fn my_exams(conn: &Connection, student: &User, q: &MyExamsQuery, now: i64) -> Res<MyExamsPage> {
    let blank = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let tab = match blank(&q.tab).as_deref() {
        None | Some("all") => None,
        Some(t) => Some(ExamState::parse(t).ok_or_else(|| bad("invalid_filter"))?),
    };
    let search: Option<String> = blank(&q.q).map(|s| s.chars().take(MAX_SEARCH_CHARS).collect());
    let rows = student_exams(conn, &student.id, blank(&q.subject_id).as_deref(), search.as_deref(), now)?;
    let counts = StateCounts::of(&rows);
    let chosen: Vec<StudentExamRow> = rows.into_iter().filter(|r| tab.map_or(true, |t| r.state == t)).collect();
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let offset = q.offset.unwrap_or(0).max(0) as usize;
    let total = chosen.len() as i64;
    let items = chosen.into_iter().skip(offset).take(limit).collect();
    Ok(MyExamsPage { items, total, counts })
}

pub async fn my_exams_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<MyExamsQuery>) -> Res<Json<MyExamsPage>> {
    let student = require_role(&s, &h, "student")?;
    my_exams(&*lock(&s)?, &student, &q, now_ms()).map(Json)
}

// ───────── GET /student/overview ─────────

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct ResumeCard {
    pub attempt_id: String,
    pub assessment_id: String,
    pub title: String,
    pub ends_at: i64,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct ClosingSoon {
    pub id: String,
    pub title: String,
    pub subject_name: String,
    pub closes_at: i64,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct OverviewExams {
    pub open: i64,
    pub upcoming: i64,
    /// Running attempts the student can still resume (`visible` rows only): the same exams as "My exams" `in_progress`
    /// minus the ones the student can no longer open, so the sidebar badge never counts something it cannot lead to.
    pub in_progress: i64,
    /// Startable now and closing within [`CLOSING_SOON_MS`], soonest first, at most [`CLOSING_SOON_MAX`].
    pub closing_soon: Vec<ClosingSoon>,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct RecentResult {
    pub attempt_id: String,
    pub assessment_id: String,
    pub title: String,
    /// `null` while the result is withheld (`released: false`).
    pub score: Option<f64>,
    pub total: f64,
    pub percent: Option<f64>,
    pub passed: Option<bool>,
    pub released: bool,
    pub submitted_at: i64,
    /// Written answers still waiting for a grade (`null` while withheld).
    pub pending: Option<i64>,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct CourseCard {
    pub course_id: String,
    pub title: String,
    pub subject_name: String,
    /// Completed lessons in percent of the course's lessons, two decimals.
    pub percent: f64,
    /// The first lesson (by position) the student has not completed.
    pub next_lesson_id: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct NextLive {
    pub id: String,
    pub title: String,
    pub subject_id: String,
    pub subject_name: String,
    pub starts_at: i64,
    pub duration_min: i64,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct Checklist {
    /// The student has chosen their institution (and level): `student_placement`.
    pub placement: bool,
    /// The student is enrolled in at least one subject that is switched on.
    pub enrolled: bool,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct StudentOverview {
    /// Enrolments in subjects that are switched on (subject AND institution active).
    pub enrolled: i64,
    /// COUNT of the student's unread notifications.
    pub unread: i64,
    /// The running attempt that ends first, among exams the student can still open (`visible`).
    pub resume: Option<ResumeCard>,
    pub exams: OverviewExams,
    /// The student's submitted attempts with written answers still waiting for a grade, counted only where the result is
    /// already released (a withheld result says nothing about its grading, like the attempt page).
    pub awaiting_grading: i64,
    /// The newest three submitted attempts.
    pub recent_results: Vec<RecentResult>,
    /// Up to four courses the student has started (at least one lesson done) and not finished, newest activity first.
    pub courses: Vec<CourseCard>,
    /// The soonest scheduled live session that has not ended, shown to students, in an enrolled subject or by a followed
    /// teacher.
    pub next_live: Option<NextLive>,
    pub checklist: Checklist,
}

pub(crate) const CLOSING_SOON_MS: i64 = 48 * 3_600_000;
pub(crate) const CLOSING_SOON_MAX: usize = 5;
const RECENT_RESULTS: i64 = 3;
const COURSE_CARDS: usize = 4;

fn count(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Res<i64> {
    conn.query_row(sql, p, |r| r.get(0)).map_err(db_err)
}

pub(crate) fn student_overview(conn: &Connection, student: &User, now: i64) -> Res<StudentOverview> {
    let id = student.id.as_str();
    let enrolled = count(
        conn,
        &format!("SELECT count(*) FROM subject_enrollments e JOIN subjects s ON s.id = e.subject_id WHERE e.student_id = ?1 AND {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE}"),
        params![id],
    )?;
    let unread = count(conn, "SELECT count(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL", params![id])?;

    // the same scope and states as "My exams" (rows come in list order: running ones first, then open by closing time)
    let rows = student_exams(conn, id, None, None, now)?;
    let counts = StateCounts::of(&rows);
    // a running attempt on an exam the student can no longer open (unenrolled, hidden, archived) cannot be resumed
    // (`start_attempt` refuses it), so it is neither offered as "resume" nor counted in `exams.in_progress`, which feeds the
    // sidebar badge ("things to act on"); "My exams" still lists the row and its tab count includes it
    let resumable = rows.iter().filter(|r| r.state == ExamState::InProgress && r.visible).count() as i64;
    let resume = rows.iter().find(|r| r.state == ExamState::InProgress && r.visible).and_then(|r| {
        r.in_progress_attempt.as_ref().map(|run| ResumeCard { attempt_id: run.attempt_id.clone(), assessment_id: r.id.clone(), title: r.title.clone(), ends_at: run.ends_at })
    });
    let closing_soon = rows
        .iter()
        .filter(|r| r.state == ExamState::Open)
        .filter_map(|r| r.closes_at.filter(|c| *c > now && *c <= now + CLOSING_SOON_MS).map(|c| (r, c)))
        .take(CLOSING_SOON_MAX)
        .map(|(r, c)| ClosingSoon { id: r.id.clone(), title: r.title.clone(), subject_name: r.subject_name.clone(), closes_at: c })
        .collect();

    let awaiting_grading = conn
        .prepare(
            "SELECT a.release_mode, a.status, a.closes_at FROM attempts t JOIN assessments a ON a.id = t.assessment_id
             WHERE t.student_id = ?1 AND t.status = 'submitted' AND t.pending > 0",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| Ok(is_released(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?, r.get(2)?, now)))
        .map_err(db_err)?
        .collect::<Result<Vec<bool>, _>>()
        .map_err(db_err)?
        .into_iter()
        .filter(|released| *released)
        .count() as i64;

    let recent_results = conn
        .prepare(
            "SELECT t.id, a.id, a.title, t.score, a.total_points, t.pending, COALESCE(t.submitted_at, t.started_at), a.release_mode, a.status, a.closes_at, a.pass_mark
             FROM attempts t JOIN assessments a ON a.id = t.assessment_id
             WHERE t.student_id = ?1 AND t.status = 'submitted'
             ORDER BY COALESCE(t.submitted_at, t.started_at) DESC, t.id DESC LIMIT ?2",
        )
        .map_err(db_err)?
        .query_map(params![id, RECENT_RESULTS], |r| {
            let (score, total, pending): (f64, f64, i64) = (r.get(3)?, r.get(4)?, r.get(5)?);
            let released = is_released(&r.get::<_, String>(7)?, &r.get::<_, String>(8)?, r.get(9)?, now);
            let pass_mark: Option<f64> = r.get(10)?;
            Ok(RecentResult {
                attempt_id: r.get(0)?,
                assessment_id: r.get(1)?,
                title: r.get(2)?,
                score: released.then_some(score),
                total,
                percent: (released && total > 0.0).then(|| round2(score / total * 100.0)),
                passed: if released { passed_flag(score, total, pending, pass_mark) } else { None },
                released,
                submitted_at: r.get(6)?,
                pending: released.then_some(pending),
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    // courses the student can still open and has started but not finished, newest activity first: the same rows, and the same
    // percent / next lesson, as the learning hub and `GET /my/progress` (`platform_student_learning::learning_items`)
    let mut started: Vec<_> = crate::platform_student_learning::learning_items(conn, id, None, None)?
        .into_iter()
        .filter(|c| c.state == crate::platform_student_learning::LearningState::InProgress)
        .collect();
    started.sort_by(|a, b| b.last_at.cmp(&a.last_at).then_with(|| a.course_id.cmp(&b.course_id)));
    let courses = started
        .into_iter()
        .take(COURSE_CARDS)
        .map(|c| CourseCard { course_id: c.course_id, title: c.title, subject_name: c.subject_name, percent: c.percent, next_lesson_id: c.next_lesson_id })
        .collect();

    let next_live = conn
        .query_row(
            &format!(
                "SELECT l.id, l.title, l.subject_id, s.name_ar, l.starts_at, l.duration_min
                 FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id
                 WHERE {} AND l.starts_at + l.duration_min * 60000 > ?2
                   AND (l.subject_id IN (SELECT subject_id FROM subject_enrollments WHERE student_id = ?1)
                        OR l.teacher_id IN (SELECT teacher_id FROM teacher_follows WHERE student_id = ?1))
                 ORDER BY l.starts_at, l.id LIMIT 1",
                visible("l", "scheduled")
            ),
            params![id, now],
            |r| Ok(NextLive { id: r.get(0)?, title: r.get(1)?, subject_id: r.get(2)?, subject_name: r.get(3)?, starts_at: r.get(4)?, duration_min: r.get(5)? }),
        )
        .optional()
        .map_err(db_err)?;

    let placement: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM student_placement WHERE student_id = ?1)", params![id], |r| r.get(0)).map_err(db_err)?;
    Ok(StudentOverview {
        enrolled,
        unread,
        resume,
        exams: OverviewExams { open: counts.open, upcoming: counts.upcoming, in_progress: resumable, closing_soon },
        awaiting_grading,
        recent_results,
        courses,
        next_live,
        checklist: Checklist { placement, enrolled: enrolled > 0 },
    })
}

pub async fn overview_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<StudentOverview>> {
    let student = require_role(&s, &h, "student")?;
    student_overview(&*lock(&s)?, &student, now_ms()).map(Json)
}

/// The student's routes (merged into the app in `main.rs`). The enrolment helpers live with the enrolment rules in
/// `platform_learning`.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/platform/my/exams", get(my_exams_handler))
        .route("/api/platform/student/overview", get(overview_handler))
        .route("/api/platform/me/suggested-subjects", get(crate::platform_learning::suggested_subjects_handler))
        .route("/api/platform/enrollments/bulk", post(crate::platform_learning::bulk_enroll_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;
    const H: i64 = 3_600_000;

    // ───────── the pure rule ─────────

    fn facts() -> ExamFacts {
        ExamFacts {
            status: "published".into(),
            opens_at: None,
            closes_at: None,
            max_attempts: 1,
            duration_min: Some(30),
            total_points: 10.0,
            pass_mark: Some(50.0),
            release_mode: "immediate".into(),
            shown: true,
            enrolled: true,
        }
    }

    fn att(id: &str, status: &str, started_at: i64, submitted_at: Option<i64>, score: f64, pending: i64) -> AttemptFact {
        AttemptFact { id: id.into(), status: status.into(), started_at, submitted_at, score, pending }
    }

    fn view(f: &ExamFacts, a: &[AttemptFact]) -> StudentView {
        student_view(f, a, NOW)
    }

    fn sr(v: &StudentView) -> (ExamState, Option<Blocked>, bool) {
        (v.state, v.reason, v.can_start)
    }

    #[test]
    fn open_when_published_shown_enrolled_opened_and_attempts_left() {
        let v = view(&facts(), &[]);
        assert_eq!(sr(&v), (ExamState::Open, None, true));
        assert_eq!((v.phase.as_str(), v.attempts_used, v.best.clone(), v.last_attempt_id.clone()), ("published", 0, None, None));
        // the opening time reached exactly: open
        let mut f = facts();
        f.opens_at = Some(NOW);
        f.closes_at = Some(NOW + 1);
        assert_eq!(sr(&view(&f, &[])), (ExamState::Open, None, true));
    }

    #[test]
    fn a_running_attempt_wins_over_everything_and_has_no_reason() {
        let running = att("r", "in_progress", NOW - 10 * MIN, None, 0.0, 0);
        let v = view(&facts(), &[running.clone()]);
        assert_eq!(sr(&v), (ExamState::InProgress, None, false), "max 1, used 1: still in progress, never 'no attempts'");
        assert_eq!(v.in_progress, Some(RunningAttempt { attempt_id: "r".into(), ends_at: NOW - 10 * MIN + 30 * MIN }));
        // even when the exam is past its closing time, hidden or the student unenrolled: it is running
        let mut f = facts();
        f.closes_at = Some(NOW - MIN);
        f.shown = false;
        f.enrolled = false;
        let v = view(&f, &[running]);
        assert_eq!(sr(&v), (ExamState::InProgress, None, false));
        assert_eq!(v.phase, "closed");
    }

    #[test]
    fn a_running_attempt_counts_until_its_deadline_plus_grace() {
        let ends = NOW - GRACE_MS; // deadline + grace == now: still running
        let at_edge = att("r", "in_progress", ends - 30 * MIN, None, 0.0, 0);
        assert_eq!(view(&facts(), &[at_edge]).state, ExamState::InProgress);
        let past = att("r", "in_progress", ends - 30 * MIN - 1, None, 0.0, 0);
        let v = view(&facts(), &[past]);
        assert_eq!(sr(&v), (ExamState::Missed, Some(Blocked::NoAttempts), false), "overdue: no longer running, and it used the only attempt");
        assert!(v.in_progress.is_none());
        // an open-ended exam (no duration) runs for 24 hours
        let mut f = facts();
        f.duration_min = None;
        f.max_attempts = 2;
        assert_eq!(view(&f, &[att("r", "in_progress", NOW - 23 * H, None, 0.0, 0)]).state, ExamState::InProgress);
        assert_eq!(view(&f, &[att("r", "in_progress", NOW - 25 * H, None, 0.0, 0)]).state, ExamState::Open);
    }

    #[test]
    fn upcoming_until_it_opens_and_the_reason_says_what_the_student_can_do() {
        let mut f = facts();
        f.opens_at = Some(NOW + 1);
        assert_eq!(sr(&view(&f, &[])), (ExamState::Upcoming, Some(Blocked::NotOpen), false));
        f.enrolled = false;
        assert_eq!(sr(&view(&f, &[])), (ExamState::Upcoming, Some(Blocked::NotEnrolled), false), "enrolling is what helps");
        // a hidden one is not "upcoming" for anybody
        f.enrolled = true;
        f.shown = false;
        assert_eq!(sr(&view(&f, &[])), (ExamState::Missed, Some(Blocked::Closed), false));
    }

    #[test]
    fn the_effective_phase_closes_a_published_exam_at_its_closing_time() {
        let mut f = facts();
        f.closes_at = Some(NOW);
        let v = view(&f, &[]);
        assert_eq!((sr(&v), v.phase.as_str()), ((ExamState::Missed, Some(Blocked::Closed), false), "closed"), "closing time reached exactly: closed");
        f.closes_at = Some(NOW + 1);
        assert_eq!(sr(&view(&f, &[])), (ExamState::Open, None, true), "one millisecond before: open");
        // closed with a submitted attempt: done, and the reason is still 'closed'
        f.closes_at = Some(NOW - H);
        f.max_attempts = 3;
        let v = view(&f, &[att("s", "submitted", NOW - 2 * H, Some(NOW - 2 * H + MIN), 7.0, 0)]);
        assert_eq!(sr(&v), (ExamState::Done, Some(Blocked::Closed), false));
        // closed / archived by hand
        for status in ["closed", "archived"] {
            let mut f = facts();
            f.status = status.into();
            let v = view(&f, &[]);
            assert_eq!((sr(&v), v.phase.as_str()), ((ExamState::Missed, Some(Blocked::Closed), false), status));
        }
    }

    #[test]
    fn used_up_attempts_with_only_expired_ones_is_missed() {
        let mut f = facts();
        f.max_attempts = 2;
        let a = [att("e1", "expired", NOW - 3 * H, None, 0.0, 0), att("e2", "expired", NOW - 2 * H, None, 0.0, 0)];
        let v = view(&f, &a);
        assert_eq!(sr(&v), (ExamState::Missed, Some(Blocked::NoAttempts), false));
        assert_eq!((v.attempts_used, v.best.clone(), v.last_attempt_id.as_deref()), (2, None, Some("e2")));
        // one expired, one left: open again
        assert_eq!(sr(&view(&f, &a[..1])), (ExamState::Open, None, true));
    }

    #[test]
    fn a_submitted_attempt_is_done_unless_another_one_can_be_started() {
        let mut f = facts();
        let sub = att("s", "submitted", NOW - H, Some(NOW - H + 5 * MIN), 6.0, 0);
        assert_eq!(sr(&view(&f, &[sub.clone()])), (ExamState::Done, Some(Blocked::NoAttempts), false));
        f.max_attempts = 2;
        assert_eq!(sr(&view(&f, &[sub.clone()])), (ExamState::Open, None, true), "open beats done: there is another attempt");
        f.enrolled = false;
        assert_eq!(sr(&view(&f, &[sub])), (ExamState::Done, Some(Blocked::NotEnrolled), false));
    }

    #[test]
    fn not_enrolled_is_never_startable() {
        let mut f = facts();
        f.enrolled = false;
        assert_eq!(sr(&view(&f, &[])), (ExamState::Missed, Some(Blocked::NotEnrolled), false));
    }

    #[test]
    fn the_reason_reports_what_nobody_can_change_first() {
        // every blocker at once: closed first
        let mut f = facts();
        f.status = "closed".into();
        f.enrolled = false;
        f.opens_at = Some(NOW + H);
        let used = [att("e", "expired", NOW - H, None, 0.0, 0)];
        assert_eq!(view(&f, &used).reason, Some(Blocked::Closed));
        f.status = "published".into();
        f.closes_at = Some(NOW + 2 * H);
        f.opens_at = None;
        assert_eq!(view(&f, &used).reason, Some(Blocked::NoAttempts), "then the attempts");
        assert_eq!(view(&f, &[]).reason, Some(Blocked::NotEnrolled), "then the enrolment");
        f.enrolled = true;
        f.opens_at = Some(NOW + H);
        assert_eq!(view(&f, &[]).reason, Some(Blocked::NotOpen), "then the opening time");
    }

    #[test]
    fn the_best_attempt_is_the_highest_score_newest_on_a_tie_and_withheld_while_embargoed() {
        let mut f = facts();
        f.max_attempts = 5;
        let a = [
            att("a", "submitted", NOW - 5 * H, Some(NOW - 5 * H), 4.0, 0),
            att("b", "submitted", NOW - 4 * H, Some(NOW - 4 * H), 8.0, 0),
            att("c", "submitted", NOW - 3 * H, Some(NOW - 3 * H), 8.0, 0),
            att("d", "submitted", NOW - 2 * H, Some(NOW - 2 * H), 3.0, 1),
            att("e", "expired", NOW - H, None, 0.0, 0),
        ];
        let v = view(&f, &a);
        assert_eq!(
            v.best,
            Some(BestAttempt { attempt_id: "c".into(), score: Some(8.0), total: 10.0, percent: Some(80.0), passed: Some(true), released: true, pending: Some(0) }),
            "8 twice: the newer one"
        );
        assert_eq!((v.last_attempt_id.as_deref(), v.last_submitted_at), (Some("e"), Some(NOW - 2 * H)));
        // below the pass mark / still pending
        let low = view(&f, &a[..1]).best.unwrap();
        assert_eq!((low.passed, low.percent), (Some(false), Some(40.0)));
        let pend = view(&f, &a[3..4]).best.unwrap();
        assert_eq!((pend.passed, pend.pending), (None, Some(1)), "no verdict while written answers wait");
        // after_close before the exam closes: nothing about the outcome, and the NEWEST attempt, not the best one
        f.release_mode = "after_close".into();
        f.closes_at = Some(NOW + H);
        let v = view(&f, &a);
        assert_eq!(
            v.best,
            Some(BestAttempt { attempt_id: "d".into(), score: None, total: 10.0, percent: None, passed: None, released: false, pending: None })
        );
        // released once the closing time is reached
        f.closes_at = Some(NOW);
        assert_eq!(view(&f, &a).best.unwrap().attempt_id, "c");
        // a total of 0 (every question voided) has no percentage
        let mut z = facts();
        z.total_points = 0.0;
        assert_eq!(view(&z, &a[..1]).best.unwrap().percent, None);
    }

    // ───────── the student's scope, from the database ─────────

    struct W {
        conn: Connection,
        teacher: User,
        me: User,
        other: User,
    }

    /// i1 (active) with s1, s2 (I am enrolled in both) and s3 (not enrolled); the teacher is approved for all three.
    fn world() -> W {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0), ('s2','i1','شبكات',1,0), ('s3','i1','رياضيات',1,0);",
        )
        .unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let me = insert_test_user(&conn, "me@x.com", "student", "active");
        let other = insert_test_user(&conn, "other@x.com", "student", "active");
        for s in ["s1", "s2", "s3"] {
            conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at) VALUES (?1,?2,'approved',0)", params![teacher.id, s]).unwrap();
        }
        for s in ["s1", "s2"] {
            enroll(&conn, &me, s);
        }
        enroll(&conn, &other, "s1");
        W { conn, teacher, me, other }
    }

    fn enroll(conn: &Connection, u: &User, subject: &str) {
        conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,?2,0)", params![u.id, subject]).unwrap();
    }

    /// A published exam of the teacher (no window, 1 attempt, 30 minutes, 10 points, pass mark 50 %).
    fn exam(w: &W, id: &str, subject: &str, title: &str) {
        w.conn
            .execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,duration_min,max_attempts,pass_mark,status,created_at,updated_at)
                 VALUES (?1,?2,?2,?3,?4,?5,2,10,30,1,50,'published',0,0)",
                params![id, w.teacher.id, subject, title, QUESTIONS],
            )
            .unwrap();
    }

    const QUESTIONS: &str = r#"[{"id":"q1","type":"single_choice","stem":"STEM-SECRET","options":["a","b"],"answer":"B","analysis":"ANALYSIS-SECRET","score":5},{"id":"q2","type":"short_answer","stem":"STEM-2","options":[],"answer":"MODEL-SECRET","analysis":"","score":5}]"#;

    fn set(w: &W, id: &str, assignments: &str) {
        w.conn.execute(&format!("UPDATE assessments SET {assignments} WHERE id = ?1"), params![id]).unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(w: &W, id: &str, exam: &str, who: &User, status: &str, started: i64, submitted: Option<i64>, score: f64, pending: i64) {
        w.conn
            .execute(
                "INSERT INTO attempts(id,assessment_id,student_id,started_at,submitted_at,status,score,pending) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, exam, who.id, started, submitted, status, score, pending],
            )
            .unwrap();
    }

    fn list(w: &W, who: &User, q: MyExamsQuery) -> MyExamsPage {
        my_exams(&w.conn, who, &q, NOW).unwrap()
    }

    fn all(w: &W, who: &User) -> MyExamsPage {
        list(w, who, MyExamsQuery { limit: Some(100), ..Default::default() })
    }

    fn row<'a>(p: &'a MyExamsPage, id: &str) -> &'a StudentExamRow {
        p.items.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("{id} is listed"))
    }

    fn ids(p: &MyExamsPage) -> Vec<&str> {
        p.items.iter().map(|r| r.id.as_str()).collect()
    }

    /// One exam per state (and the rows my attempts keep), in my enrolled subjects.
    fn seeded() -> W {
        let w = world();
        exam(&w, "open", "s1", "مفتوح");
        exam(&w, "upcoming", "s1", "قادم");
        set(&w, "upcoming", &format!("opens_at = {}", NOW + H));
        exam(&w, "running", "s2", "جار");
        attempt(&w, "a-run", "running", &w.me, "in_progress", NOW - 5 * MIN, None, 0.0, 0);
        exam(&w, "done", "s2", "منجز");
        attempt(&w, "a-done", "done", &w.me, "submitted", NOW - 2 * H, Some(NOW - 2 * H + 10 * MIN), 7.0, 0);
        exam(&w, "missed", "s1", "فائت");
        set(&w, "missed", &format!("closes_at = {}", NOW - H));
        w
    }

    #[test]
    fn every_state_is_listed_and_the_counts_agree_with_the_items_of_each_tab() {
        let w = seeded();
        let p = all(&w, &w.me);
        let states: Vec<(&str, ExamState)> = p.items.iter().map(|r| (r.id.as_str(), r.state)).collect();
        assert_eq!(
            states,
            vec![("running", ExamState::InProgress), ("open", ExamState::Open), ("upcoming", ExamState::Upcoming), ("done", ExamState::Done), ("missed", ExamState::Missed)],
            "tab=all: in progress, open, upcoming, done, missed"
        );
        assert_eq!(p.counts, StateCounts { open: 1, upcoming: 1, in_progress: 1, done: 1, missed: 1 });
        assert_eq!(p.total, 5);
        for (tab, n) in [("open", 1), ("upcoming", 1), ("in_progress", 1), ("done", 1), ("missed", 1), ("all", 5)] {
            let t = list(&w, &w.me, MyExamsQuery { tab: Some(tab.into()), ..Default::default() });
            assert_eq!((t.items.len() as i64, t.total), (n, n), "{tab}");
            assert_eq!(t.counts, p.counts, "{tab}: counts ignore the tab");
            assert!(t.items.iter().all(|r| tab == "all" || ExamState::parse(tab) == Some(r.state)), "{tab}");
        }
        // the row shape
        let r = row(&p, "running");
        assert_eq!(r.in_progress_attempt, Some(RunningAttempt { attempt_id: "a-run".into(), ends_at: NOW - 5 * MIN + 30 * MIN }));
        assert_eq!((r.can_start, r.reason, r.visible, r.attempts_used, r.last_attempt_id.as_deref()), (false, None, true, 1, Some("a-run")));
        let o = row(&p, "open");
        assert_eq!((o.can_start, o.reason, o.phase.as_str(), o.teacher_name.as_str(), o.subject_name.as_str()), (true, None, "published", "T", "برمجة"));
        assert_eq!((row(&p, "upcoming").reason, row(&p, "missed").reason, row(&p, "missed").phase.as_str()), (Some(Blocked::NotOpen), Some(Blocked::Closed), "closed"));
        let d = row(&p, "done");
        assert_eq!(d.best.as_ref().map(|b| (b.attempt_id.as_str(), b.score, b.percent, b.passed)), Some(("a-done", Some(7.0), Some(70.0), Some(true))));
        assert_eq!(d.reason, Some(Blocked::NoAttempts));
    }

    #[test]
    fn the_json_row_is_the_agreed_contract_and_never_carries_questions() {
        let w = seeded();
        let p = all(&w, &w.me);
        let v = serde_json::to_value(row(&p, "done")).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "attempts_used", "best", "can_start", "closes_at", "duration_min", "id", "in_progress_attempt", "last_attempt_id", "max_attempts", "opens_at", "pass_mark",
                "phase", "question_count", "reason", "release_mode", "state", "subject_id", "subject_name", "teacher_name", "title", "total_points", "visible"
            ]
        );
        assert_eq!(v["state"], json!("done"));
        assert_eq!(v["reason"], json!("no_attempts"));
        assert_eq!(
            v["best"],
            json!({"attempt_id": "a-done", "score": 7.0, "total": 10.0, "percent": 70.0, "passed": true, "released": true, "pending": 0})
        );
        let page = serde_json::to_value(&p).unwrap();
        assert_eq!(page["counts"], json!({"open": 1, "upcoming": 1, "in_progress": 1, "done": 1, "missed": 1}));
        assert_eq!(page["total"], json!(5));
        let text = page.to_string();
        for secret in ["STEM-SECRET", "ANALYSIS-SECRET", "MODEL-SECRET", "\"questions\"", "\"answer\""] {
            assert!(!text.contains(secret), "{secret} never leaves the server in this list");
        }
    }

    #[test]
    fn each_state_has_its_own_order_and_ties_are_stable() {
        let w = world();
        // open: closing soonest first, no closing time last
        for (id, closes) in [("o-late", Some(NOW + 9 * H)), ("o-none", None), ("o-soon", Some(NOW + H))] {
            exam(&w, id, "s1", id);
            if let Some(c) = closes {
                set(&w, id, &format!("closes_at = {c}"));
            }
        }
        // upcoming: opening soonest first
        for (id, opens) in [("u-late", NOW + 5 * H), ("u-soon", NOW + 2 * H)] {
            exam(&w, id, "s1", id);
            set(&w, id, &format!("opens_at = {opens}"));
        }
        // done: most recently handed in first
        for (id, at) in [("d-old", NOW - 9 * H), ("d-new", NOW - H)] {
            exam(&w, id, "s2", id);
            attempt(&w, &format!("a-{id}"), id, &w.me, "submitted", at - MIN, Some(at), 5.0, 0);
        }
        // missed: most recently closed first, no closing time last
        for (id, closes) in [("m-old", Some(NOW - 9 * H)), ("m-none", None), ("m-new", Some(NOW - H))] {
            exam(&w, id, "s2", id);
            match closes {
                Some(c) => set(&w, id, &format!("closes_at = {c}")),
                None => attempt(&w, &format!("a-{id}"), id, &w.me, "expired", NOW - 3 * H, None, 0.0, 0),
            }
        }
        // in progress: the one that ends first
        for (id, started) in [("r-late", NOW - MIN), ("r-soon", NOW - 20 * MIN)] {
            exam(&w, id, "s1", id);
            attempt(&w, &format!("a-{id}"), id, &w.me, "in_progress", started, None, 0.0, 0);
        }
        let p = all(&w, &w.me);
        assert_eq!(ids(&p), vec!["r-soon", "r-late", "o-soon", "o-late", "o-none", "u-soon", "u-late", "d-new", "d-old", "m-new", "m-old", "m-none"]);
        // a single tab keeps the same order
        assert_eq!(ids(&list(&w, &w.me, MyExamsQuery { tab: Some("open".into()), ..Default::default() })), vec!["o-soon", "o-late", "o-none"]);
        assert_eq!(ids(&list(&w, &w.me, MyExamsQuery { tab: Some("missed".into()), ..Default::default() })), vec!["m-new", "m-old", "m-none"]);
        // equal keys: by title, then id
        exam(&w, "o-b", "s1", "ب");
        exam(&w, "o-a", "s1", "أ");
        set(&w, "o-b", &format!("closes_at = {}", NOW + 3 * H));
        set(&w, "o-a", &format!("closes_at = {}", NOW + 3 * H));
        let open = ids(&list(&w, &w.me, MyExamsQuery { tab: Some("open".into()), ..Default::default() })).into_iter().map(String::from).collect::<Vec<_>>();
        assert_eq!(open, vec!["o-soon", "o-a", "o-b", "o-late", "o-none"]);
    }

    #[test]
    fn paging_and_limits() {
        let w = world();
        for i in 0..130 {
            exam(&w, &format!("e{i:03}"), "s1", &format!("امتحان {i:03}"));
        }
        let first = list(&w, &w.me, MyExamsQuery::default());
        assert_eq!((first.items.len(), first.total), (25, 130), "default page size 25");
        assert_eq!(list(&w, &w.me, MyExamsQuery { limit: Some(1000), ..Default::default() }).items.len(), 100, "at most 100");
        assert_eq!(list(&w, &w.me, MyExamsQuery { limit: Some(0), ..Default::default() }).items.len(), 1, "at least 1");
        assert_eq!(list(&w, &w.me, MyExamsQuery { limit: Some(-5), offset: Some(-3), ..Default::default() }).items[0].id, first.items[0].id, "negative offset = 0");
        // pages are disjoint and cover everything once
        let mut seen = std::collections::HashSet::new();
        for page in 0..6 {
            let p = list(&w, &w.me, MyExamsQuery { limit: Some(25), offset: Some(page * 25), ..Default::default() });
            assert_eq!(p.total, 130);
            for r in &p.items {
                assert!(seen.insert(r.id.clone()), "{} on two pages", r.id);
            }
        }
        assert_eq!(seen.len(), 130);
        let beyond = list(&w, &w.me, MyExamsQuery { offset: Some(500), ..Default::default() });
        assert_eq!((beyond.items.len(), beyond.total, beyond.counts.open), (0, 130, 130));
    }

    #[test]
    fn an_unknown_tab_is_a_400_and_blank_filters_are_ignored() {
        let w = seeded();
        for bad_tab in ["closed", "OPEN", "x", "in-progress"] {
            let e = my_exams(&w.conn, &w.me, &MyExamsQuery { tab: Some(bad_tab.into()), ..Default::default() }, NOW).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_filter")), (axum::http::StatusCode::BAD_REQUEST, true), "{bad_tab}");
        }
        let blank = list(&w, &w.me, MyExamsQuery { tab: Some(" ".into()), subject_id: Some("".into()), q: Some("  ".into()), ..Default::default() });
        assert_eq!(blank.total, 5);
        assert_eq!(list(&w, &w.me, MyExamsQuery { tab: Some("all".into()), ..Default::default() }).total, 5);
    }

    #[test]
    fn subject_filter_and_title_search_with_literal_wildcards() {
        let w = world();
        for (id, title) in [("m", "Math 101"), ("pct", "100% sure"), ("us", "a_b test"), ("plain", "axb test"), ("bs", "back\\slash")] {
            exam(&w, id, "s1", title);
        }
        exam(&w, "net", "s2", "Networks");
        let q = |s: &str| {
            let mut v: Vec<String> = ids(&list(&w, &w.me, MyExamsQuery { q: Some(s.into()), ..Default::default() })).into_iter().map(String::from).collect();
            v.sort();
            v
        };
        assert_eq!(q("MATH"), vec!["m"], "case-insensitive");
        assert_eq!(q("%"), vec!["pct"], "% is a literal");
        assert_eq!(q("_"), vec!["us"], "_ is a literal");
        assert_eq!(q("\\"), vec!["bs"], "\\ is a literal");
        assert_eq!(q("a_b"), vec!["us"], "`a_b` does not match `axb`");
        let s2 = list(&w, &w.me, MyExamsQuery { subject_id: Some("s2".into()), ..Default::default() });
        assert_eq!((ids(&s2), s2.counts.open), (vec!["net"], 1), "counts follow the subject filter");
        let searched = list(&w, &w.me, MyExamsQuery { q: Some("test".into()), ..Default::default() });
        assert_eq!((searched.total, searched.counts.open), (2, 2), "counts follow the search");
        // a very long search is cut, not refused
        assert_eq!(list(&w, &w.me, MyExamsQuery { q: Some("x".repeat(5000)), ..Default::default() }).total, 0);
    }

    #[test]
    fn other_students_attempts_never_count() {
        let w = seeded();
        // the other student is in s1 too: every attempt of theirs on my exams, and on exams I cannot see at all
        attempt(&w, "o1", "open", &w.other, "submitted", NOW - H, Some(NOW - H + MIN), 10.0, 0);
        attempt(&w, "o2", "upcoming", &w.other, "in_progress", NOW - MIN, None, 0.0, 0);
        exam(&w, "theirs-hidden", "s3", "خاص بغيري");
        attempt(&w, "o3", "theirs-hidden", &w.other, "submitted", NOW - H, Some(NOW - H), 9.0, 0);
        set(&w, "theirs-hidden", "status = 'archived'");
        let p = all(&w, &w.me);
        assert!(!ids(&p).contains(&"theirs-hidden"), "an exam is never in my scope because of somebody else's attempt");
        let o = row(&p, "open");
        assert_eq!((o.state, o.attempts_used, o.best.clone(), o.last_attempt_id.clone()), (ExamState::Open, 0, None, None));
        assert_eq!(row(&p, "upcoming").in_progress_attempt, None);
        assert_eq!(p.counts, StateCounts { open: 1, upcoming: 1, in_progress: 1, done: 1, missed: 1 });
        let text = serde_json::to_string(&p).unwrap();
        for theirs in ["o1", "o2", "o3"] {
            assert!(!text.contains(&format!("\"{theirs}\"")), "{theirs} never appears in my list");
        }
        // and the other student sees their own, not mine
        let theirs = all(&w, &w.other);
        assert_eq!(row(&theirs, "open").best.as_ref().map(|b| b.attempt_id.as_str()), Some("o1"));
        assert!(!ids(&theirs).contains(&"done") && !ids(&theirs).contains(&"running"), "s2 is not theirs and they have no attempt there");
        assert!(ids(&theirs).contains(&"theirs-hidden"));
    }

    #[test]
    fn rows_kept_by_my_attempts_lead_to_results_never_to_a_start() {
        let w = world();
        // a hidden subject (my submitted attempt), a hidden institution would do the same
        exam(&w, "hidden", "s2", "مخفي");
        attempt(&w, "a-hidden", "hidden", &w.me, "submitted", NOW - H, Some(NOW - H), 8.0, 0);
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's2'", []).unwrap();
        // a subject I am not enrolled in any more, the exam still open with attempts left
        exam(&w, "left", "s3", "تركت المادة");
        set(&w, "left", "max_attempts = 3");
        attempt(&w, "a-left", "left", &w.me, "submitted", NOW - H, Some(NOW - H), 5.0, 0);
        // an archived exam
        exam(&w, "arch", "s1", "مؤرشف");
        attempt(&w, "a-arch", "arch", &w.me, "expired", NOW - 9 * H, None, 0.0, 0);
        set(&w, "arch", "status = 'archived'");
        // an exam of a subject I am not enrolled in, never sat: not mine at all
        exam(&w, "gone", "s3", "لا شأن لي به");
        let p = all(&w, &w.me);
        let h = row(&p, "hidden");
        assert_eq!((h.visible, h.state, h.reason, h.can_start), (false, ExamState::Done, Some(Blocked::Closed), false));
        assert_eq!(h.best.as_ref().map(|b| b.score), Some(Some(8.0)), "my result is still reachable");
        let l = row(&p, "left");
        assert_eq!((l.visible, l.state, l.reason, l.can_start), (false, ExamState::Done, Some(Blocked::NotEnrolled), false));
        let a = row(&p, "arch");
        assert_eq!((a.visible, a.phase.as_str(), a.state, a.reason, a.can_start), (false, "archived", ExamState::Missed, Some(Blocked::Closed), false));
        assert!(!ids(&p).contains(&"gone"), "s3 is not mine and I never sat it");
        // institution switched off: the same
        w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's2'", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        let p = all(&w, &w.me);
        assert!(p.items.iter().all(|r| !r.visible && !r.can_start), "a hidden institution hides every exam");
        assert_eq!(p.items.len(), 3, "only the rows my attempts keep");
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("STEM-SECRET") && !text.contains("MODEL-SECRET") && !text.contains("ANALYSIS-SECRET"));
    }

    #[test]
    fn an_overdue_attempt_is_settled_before_the_states_are_read() {
        let w = world();
        exam(&w, "saved", "s1", "حفظ");
        exam(&w, "silent", "s1", "بلا حفظ");
        let started = NOW - 30 * MIN - GRACE_MS - 1;
        attempt(&w, "a-saved", "saved", &w.me, "in_progress", started, None, 0.0, 0);
        attempt(&w, "a-silent", "silent", &w.me, "in_progress", started, None, 0.0, 0);
        w.conn.execute("UPDATE attempts SET answers = '{\"q1\":\"B\"}', saved_at = ?1 WHERE id = 'a-saved'", params![started + MIN]).unwrap();
        let p = all(&w, &w.me);
        let st = |id: &str| -> String { w.conn.query_row("SELECT status FROM attempts WHERE id = ?1", params![id], |r| r.get(0)).unwrap() };
        assert_eq!((st("a-saved"), st("a-silent")), ("submitted".to_string(), "expired".to_string()));
        let s = row(&p, "saved");
        assert_eq!((s.state, s.best.as_ref().map(|b| b.score)), (ExamState::Done, Some(Some(5.0))), "the autosaved answer was graded");
        assert_eq!((row(&p, "silent").state, row(&p, "silent").reason), (ExamState::Missed, Some(Blocked::NoAttempts)));
        assert_eq!(p.counts.in_progress, 0);
    }

    #[test]
    fn an_embargoed_best_result_is_null_until_the_exam_closes() {
        let w = world();
        exam(&w, "later", "s1", "نتيجة لاحقة");
        set(&w, "later", &format!("release_mode = 'after_close', closes_at = {}", NOW + H));
        attempt(&w, "a1", "later", &w.me, "submitted", NOW - H, Some(NOW - H), 9.0, 0);
        let b = row(&all(&w, &w.me), "later").best.clone().unwrap();
        assert_eq!((b.score, b.percent, b.passed, b.pending, b.released, b.total), (None, None, None, None, false, 10.0));
        set(&w, "later", "status = 'closed'");
        let b = row(&all(&w, &w.me), "later").best.clone().unwrap();
        assert_eq!((b.score, b.percent, b.passed, b.released), (Some(9.0), Some(90.0), Some(true), true), "closed by hand: released");
    }

    #[test]
    fn the_exam_page_and_the_available_list_use_the_same_verdict() {
        let w = seeded();
        let info = crate::platform_exams::get_info(&w.conn, &w.me.id, "upcoming").unwrap();
        let v = exam_view(&w.conn, &w.me.id, &info, NOW).unwrap();
        assert_eq!((v.state, v.reason, v.phase.as_str()), (ExamState::Upcoming, Some(Blocked::NotOpen), "published"));
        // the exam page of a subject I am not enrolled in
        exam(&w, "browse", "s3", "تصفح");
        let info = crate::platform_exams::get_info(&w.conn, &w.me.id, "browse").unwrap();
        let v = exam_view(&w.conn, &w.me.id, &info, NOW).unwrap();
        assert_eq!((v.state, v.reason, v.can_start), (ExamState::Missed, Some(Blocked::NotEnrolled), false));
        // "available": running, open and upcoming only; running first, then by closing time
        set(&w, "upcoming", &format!("closes_at = {}", NOW + 2 * H));
        set(&w, "open", &format!("closes_at = {}", NOW + 5 * H));
        let avail: Vec<String> = crate::platform_exams::list_available_at(&w.conn, &w.me.id, NOW).unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(avail, vec!["running", "upcoming", "open"], "no done, no missed (closed by time), no exam of a subject I am not in");
    }
    /// An exam an admin made himself has no owning teacher (`teacher_id` NULL, `created_by` = the admin): it must be as
    /// visible to enrolled students as a teacher's exam (the ownerless branch of `exam_standing`), carry the admin's name,
    /// and count in the overview.
    #[test]
    fn an_exam_made_by_an_admin_is_listed_startable_and_counted() {
        let w = world();
        let admin = insert_test_user(&w.conn, "adm@x.com", "admin", "active");
        w.conn.execute("UPDATE users SET full_name = 'المدير العام' WHERE id = ?1", params![admin.id]).unwrap();
        w.conn
            .execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,duration_min,max_attempts,pass_mark,status,closes_at,created_at,updated_at)
                 VALUES ('adm-exam',NULL,?1,'s1','امتحان المدير',?2,2,10,30,1,50,'published',?3,0,0)",
                params![admin.id, QUESTIONS, NOW + 10 * H],
            )
            .unwrap();
        // an enrolled student: open, visible, startable, with the admin's name; the overview counts it as closing soon
        let p = all(&w, &w.me);
        let r = row(&p, "adm-exam");
        assert_eq!((r.state, r.can_start, r.visible, r.reason, r.teacher_name.as_str()), (ExamState::Open, true, true, None, "المدير العام"));
        let o = overview(&w, &w.me);
        assert_eq!((o.exams.open, o.exams.closing_soon.iter().map(|c| c.id.as_str()).collect::<Vec<_>>()), (1, vec!["adm-exam"]));
        // the exam page and the older "available" list agree
        let info = crate::platform_exams::get_info(&w.conn, &w.me.id, "adm-exam").unwrap();
        assert_eq!((info.teacher_id.clone(), info.teacher_name.as_str()), (None, "المدير العام"));
        let v = exam_view(&w.conn, &w.me.id, &info, NOW).unwrap();
        assert_eq!((v.state, v.can_start), (ExamState::Open, true));
        let avail: Vec<String> = crate::platform_exams::list_available_at(&w.conn, &w.me.id, NOW).unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(avail, vec!["adm-exam"]);
        // a student of another subject does not see it; once they have an attempt they keep a row for their results only
        let outsider = insert_test_user(&w.conn, "out@x.com", "student", "active");
        assert!(all(&w, &outsider).items.is_empty());
        attempt(&w, "a-out", "adm-exam", &outsider, "submitted", NOW - 2 * H, Some(NOW - H), 5.0, 0);
        let kept = all(&w, &outsider);
        let r = row(&kept, "adm-exam");
        assert_eq!((r.visible, r.can_start, r.teacher_name.as_str(), r.state), (false, false, "المدير العام", ExamState::Done));
    }

    // ───────── the overview ─────────

    fn course(w: &W, id: &str, subject: &str, status: &str) {
        w.conn.execute("INSERT INTO courses(id,teacher_id,subject_id,title,status,created_at,updated_at) VALUES (?1,?2,?3,?1,?4,0,0)", params![id, w.teacher.id, subject, status]).unwrap();
    }

    fn lesson(w: &W, id: &str, course: &str, position: i64) {
        w.conn
            .execute(
                "INSERT INTO lessons(id,course_id,position,title,video_url,video_kind,created_at) VALUES (?1,?2,?3,?1,'https://x.example/v','link',0)",
                params![id, course, position],
            )
            .unwrap();
    }

    fn done_lesson(w: &W, who: &User, lesson: &str, at: i64) {
        w.conn.execute("INSERT INTO lesson_progress(student_id,lesson_id,completed_at) VALUES (?1,?2,?3)", params![who.id, lesson, at]).unwrap();
    }

    fn live(w: &W, id: &str, teacher: &User, subject: &str, starts: i64, minutes: i64, status: &str) {
        w.conn
            .execute(
                "INSERT INTO live_sessions(id,teacher_id,subject_id,title,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,?3,?1,?4,?5,'https://x.example/j',?6,0)",
                params![id, teacher.id, subject, starts, minutes, status],
            )
            .unwrap();
    }

    fn notice(w: &W, id: &str, who: &User, read: bool) {
        w.conn.execute("INSERT INTO notifications(id,user_id,kind,data,read_at,created_at) VALUES (?1,?2,'x','{}',?3,0)", params![id, who.id, read.then_some(1i64)]).unwrap();
    }

    fn overview(w: &W, who: &User) -> StudentOverview {
        student_overview(&w.conn, who, NOW).unwrap()
    }

    #[test]
    fn a_new_student_sees_zeros_and_the_json_shape_is_the_agreed_contract() {
        let conn = create_test_db();
        let me = insert_test_user(&conn, "new@x.com", "student", "active");
        let v = serde_json::to_value(student_overview(&conn, &me, NOW).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({
                "enrolled": 0, "unread": 0, "resume": null,
                "exams": {"open": 0, "upcoming": 0, "in_progress": 0, "closing_soon": []},
                "awaiting_grading": 0, "recent_results": [], "courses": [], "next_live": null,
                "checklist": {"placement": false, "enrolled": false}
            })
        );
    }

    #[test]
    fn enrolled_unread_and_the_checklist_count_only_my_rows() {
        let w = world();
        // a switched-off subject does not count as "enrolled"
        w.conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('off','i1','مطفأة',0,0)", []).unwrap();
        enroll(&w.conn, &w.me, "off");
        for (id, who, read) in [("n1", &w.me, false), ("n2", &w.me, false), ("n3", &w.me, true), ("n4", &w.other, false)] {
            notice(&w, id, who, read);
        }
        let o = overview(&w, &w.me);
        assert_eq!((o.enrolled, o.unread), (2, 2));
        assert_eq!(o.checklist, Checklist { placement: false, enrolled: true });
        w.conn.execute("INSERT INTO student_placement(student_id,institution_id,unit_id) VALUES (?1,'i1',NULL)", params![w.me.id]).unwrap();
        assert!(overview(&w, &w.me).checklist.placement);
        assert!(!overview(&w, &w.other).checklist.placement, "my placement is not theirs");
        assert_eq!(overview(&w, &w.other).unread, 1);
        // an institution switched off: nothing counts as enrolled any more
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        let o = overview(&w, &w.me);
        assert_eq!((o.enrolled, o.checklist.enrolled), (0, false));
    }

    #[test]
    fn the_exam_figures_follow_my_exams_and_closing_soon_is_a_48_hour_window_of_open_exams() {
        let w = seeded();
        for (id, closes) in [("c1h", NOW + H), ("c47h", NOW + 47 * H), ("c48h", NOW + 48 * H), ("c49h", NOW + 49 * H), ("c2h", NOW + 2 * H), ("c3h", NOW + 3 * H), ("c4h", NOW + 4 * H)] {
            exam(&w, id, "s1", id);
            set(&w, id, &format!("closes_at = {closes}"));
        }
        // closing soon but not startable: upcoming, used up, running
        exam(&w, "up-soon", "s1", "up-soon");
        set(&w, "up-soon", &format!("opens_at = {}, closes_at = {}", NOW + H, NOW + 5 * H));
        exam(&w, "used-soon", "s1", "used-soon");
        set(&w, "used-soon", &format!("closes_at = {}", NOW + H));
        attempt(&w, "a-used", "used-soon", &w.me, "expired", NOW - 3 * H, None, 0.0, 0);
        set(&w, "running", &format!("closes_at = {}", NOW + H));
        let o = overview(&w, &w.me);
        let page = all(&w, &w.me);
        assert_eq!((o.exams.open, o.exams.upcoming, o.exams.in_progress), (page.counts.open, page.counts.upcoming, page.counts.in_progress));
        assert_eq!((o.exams.open, o.exams.upcoming, o.exams.in_progress), (8, 2, 1));
        let soon: Vec<(&str, i64)> = o.exams.closing_soon.iter().map(|c| (c.id.as_str(), c.closes_at)).collect();
        assert_eq!(soon, vec![("c1h", NOW + H), ("c2h", NOW + 2 * H), ("c3h", NOW + 3 * H), ("c4h", NOW + 4 * H), ("c47h", NOW + 47 * H)], "soonest first, at most 5");
        assert_eq!(o.exams.closing_soon[0].subject_name, "برمجة");
        // with fewer near ones the 48-hour edge is included and 49 hours is not
        for id in ["c1h", "c2h", "c3h", "c4h"] {
            set(&w, id, "status = 'archived'");
        }
        let soon: Vec<String> = overview(&w, &w.me).exams.closing_soon.into_iter().map(|c| c.id).collect();
        assert_eq!(soon, vec!["c47h", "c48h"]);
        // resume: the running attempt, with its deadline
        assert_eq!(o.resume, Some(ResumeCard { attempt_id: "a-run".into(), assessment_id: "running".into(), title: "جار".into(), ends_at: NOW - 5 * MIN + 30 * MIN }));
    }

    #[test]
    fn resume_only_offers_an_attempt_the_student_can_still_open() {
        let w = world();
        exam(&w, "hidden-run", "s3", "مخفي");
        attempt(&w, "a-h", "hidden-run", &w.me, "in_progress", NOW - 25 * MIN, None, 0.0, 0); // ends first, but s3 is not mine
        assert_eq!(overview(&w, &w.me).resume, None);
        // it is still running, and "My exams" still lists it (its tab count includes it) ...
        let hidden = row(&all(&w, &w.me), "hidden-run").clone();
        assert_eq!((hidden.state, hidden.visible, hidden.in_progress_attempt.is_some()), (ExamState::InProgress, false, true));
        assert_eq!(all(&w, &w.me).counts.in_progress, 1);
        // ... but the overview (and so the sidebar badge) counts only what the student can act on
        assert_eq!(overview(&w, &w.me).exams.in_progress, 0, "nothing to resume: no badge");
        exam(&w, "mine-run", "s1", "لي");
        attempt(&w, "a-m", "mine-run", &w.me, "in_progress", NOW - MIN, None, 0.0, 0);
        assert_eq!(overview(&w, &w.me).resume.map(|r| r.attempt_id), Some("a-m".into()));
        assert_eq!(overview(&w, &w.me).exams.in_progress, 1, "the resumable one only");
        assert_eq!(all(&w, &w.me).counts.in_progress, 2, "the list's tab still counts both");
        // another student's running attempt is never my resume card
        let o = overview(&w, &w.other);
        assert_eq!((o.resume, o.exams.in_progress), (None, 0));
    }

    #[test]
    fn awaiting_grading_and_recent_results_respect_the_embargo_and_belong_to_me() {
        let w = world();
        exam(&w, "e-now", "s1", "فوري");
        exam(&w, "e-later", "s1", "لاحق");
        set(&w, "e-later", &format!("release_mode = 'after_close', closes_at = {}", NOW + H));
        exam(&w, "e-old", "s2", "قديم");
        exam(&w, "e-zero", "s2", "مصحح");
        set(&w, "e-now", "max_attempts = 3");
        attempt(&w, "r1", "e-now", &w.me, "submitted", NOW - 5 * H, Some(NOW - 5 * H), 6.0, 2); // waiting for a grade
        attempt(&w, "r2", "e-later", &w.me, "submitted", NOW - 2 * H, Some(NOW - 2 * H), 9.0, 1); // withheld: not counted
        attempt(&w, "r3", "e-old", &w.me, "submitted", NOW - 9 * H, Some(NOW - 9 * H), 4.0, 0);
        attempt(&w, "r4", "e-zero", &w.me, "submitted", NOW - H, Some(NOW - H), 3.0, 0);
        attempt(&w, "r5", "e-now", &w.me, "expired", NOW - MIN, None, 0.0, 0); // not a result
        attempt(&w, "x1", "e-now", &w.other, "submitted", NOW - 10, Some(NOW - 10), 10.0, 4);
        let o = overview(&w, &w.me);
        assert_eq!(o.awaiting_grading, 1, "r1 only: r2 is withheld, the others are graded, x1 is someone else's");
        let recent: Vec<&str> = o.recent_results.iter().map(|r| r.attempt_id.as_str()).collect();
        assert_eq!(recent, vec!["r4", "r2", "r1"], "the newest three submitted attempts");
        let r2 = &o.recent_results[1];
        assert_eq!((r2.score, r2.percent, r2.passed, r2.pending, r2.released, r2.total), (None, None, None, None, false, 10.0));
        let r4 = &o.recent_results[0];
        assert_eq!(
            *r4,
            RecentResult { attempt_id: "r4".into(), assessment_id: "e-zero".into(), title: "مصحح".into(), score: Some(3.0), total: 10.0, percent: Some(30.0), passed: Some(false), released: true, submitted_at: NOW - H, pending: Some(0) }
        );
        assert_eq!((o.recent_results[2].passed, o.recent_results[2].pending), (None, Some(2)), "pending: no verdict yet");
        let theirs = overview(&w, &w.other);
        assert_eq!((theirs.awaiting_grading, theirs.recent_results.len()), (1, 1));
    }

    #[test]
    fn courses_in_progress_with_the_next_lesson_newest_activity_first() {
        let w = world();
        // c1: 3 lessons (positions out of id order), lesson at position 1 done -> next is the one at position 2
        course(&w, "c1", "s1", "published");
        lesson(&w, "l-c", "c1", 3);
        lesson(&w, "l-a", "c1", 1);
        lesson(&w, "l-b", "c1", 2);
        done_lesson(&w, &w.me, "l-a", NOW - 3 * H);
        // c2: finished; c3: not started; c4: started but hidden (teacher not approved for s2 any more); c5: subject not mine
        course(&w, "c2", "s1", "published");
        lesson(&w, "l2", "c2", 1);
        done_lesson(&w, &w.me, "l2", NOW - H);
        course(&w, "c3", "s1", "published");
        lesson(&w, "l3", "c3", 1);
        course(&w, "c4", "s2", "published");
        lesson(&w, "l4", "c4", 1);
        lesson(&w, "l4b", "c4", 2);
        done_lesson(&w, &w.me, "l4", NOW - H);
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE subject_id = 's2'", []).unwrap();
        course(&w, "c5", "s3", "published");
        lesson(&w, "l5", "c5", 1);
        lesson(&w, "l5b", "c5", 2);
        done_lesson(&w, &w.me, "l5", NOW - H);
        // a draft course
        course(&w, "c6", "s1", "draft");
        lesson(&w, "l6", "c6", 1);
        lesson(&w, "l6b", "c6", 2);
        done_lesson(&w, &w.me, "l6", NOW - H);
        // the other student's progress on c3 is not mine
        done_lesson(&w, &w.other, "l3", NOW - MIN);
        let o = overview(&w, &w.me);
        assert_eq!(o.courses, vec![CourseCard { course_id: "c1".into(), title: "c1".into(), subject_name: "برمجة".into(), percent: 33.33, next_lesson_id: Some("l-b".into()) }]);
        // newest activity first, at most four
        for i in 0..5 {
            let id = format!("k{i}");
            course(&w, &id, "s1", "published");
            lesson(&w, &format!("{id}-1"), &id, 1);
            lesson(&w, &format!("{id}-2"), &id, 2);
            done_lesson(&w, &w.me, &format!("{id}-2"), NOW - (10 - i) * MIN);
        }
        let ids: Vec<String> = overview(&w, &w.me).courses.into_iter().map(|c| c.course_id).collect();
        assert_eq!(ids, vec!["k4", "k3", "k2", "k1"]);
        let k4 = overview(&w, &w.me).courses.into_iter().next().unwrap();
        assert_eq!((k4.percent, k4.next_lesson_id.as_deref()), (50.0, Some("k4-1")), "the first lesson not completed, even before a completed one");
    }

    #[test]
    fn next_live_is_the_soonest_visible_session_of_my_subjects_or_followed_teachers() {
        let w = world();
        let t2 = insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at) VALUES (?1,'s3','approved',0)", params![t2.id]).unwrap();
        live(&w, "l-ended", &w.teacher, "s1", NOW - 2 * H, 60, "scheduled");
        live(&w, "l-cancel", &w.teacher, "s1", NOW + MIN, 60, "cancelled");
        live(&w, "l-later", &w.teacher, "s1", NOW + 5 * H, 60, "scheduled");
        live(&w, "l-other", &t2, "s3", NOW + H, 60, "scheduled"); // neither my subject nor (yet) a teacher I follow
        let o = overview(&w, &w.me);
        assert_eq!(
            o.next_live,
            Some(NextLive { id: "l-later".into(), title: "l-later".into(), subject_id: "s1".into(), subject_name: "برمجة".into(), starts_at: NOW + 5 * H, duration_min: 60 })
        );
        w.conn.execute("INSERT INTO teacher_follows(student_id,teacher_id,created_at) VALUES (?1,?2,0)", params![w.me.id, t2.id]).unwrap();
        assert_eq!(overview(&w, &w.me).next_live.map(|l| l.id), Some("l-other".into()), "a followed teacher's session counts");
        // running right now (started, not ended): the soonest
        live(&w, "l-now", &w.teacher, "s2", NOW - 10 * MIN, 60, "scheduled");
        assert_eq!(overview(&w, &w.me).next_live.map(|l| l.id), Some("l-now".into()));
        // hidden: the teacher is no longer approved for s2
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's2'", params![w.teacher.id]).unwrap();
        assert_eq!(overview(&w, &w.me).next_live.map(|l| l.id), Some("l-other".into()));
        assert_eq!(overview(&w, &w.other).next_live.map(|l| l.id), Some("l-later".into()), "the other student follows nobody");
    }
}

/// The routes through the real router (`routes()`), so the role wall is checked where it is enforced.
#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, new_id, PlatformState};
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    struct H {
        app: Router,
        state: Arc<AppState>,
        tokens: HashMap<&'static str, String>,
        ids: HashMap<&'static str, String>,
    }

    fn token_for(conn: &Connection, u: &User) -> String {
        let token = format!("tok-{}", new_id());
        conn.execute(
            "INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, 0, ?3)",
            params![crate::relay::sha256_hex(&token), u.id, now_ms() + 3_600_000],
        )
        .unwrap();
        token
    }

    fn harness() -> H {
        static CFG: std::sync::Once = std::sync::Once::new();
        CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-http-test-{}", new_id()))));
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0);",
        )
        .unwrap();
        let mut tokens = HashMap::new();
        let mut ids = HashMap::new();
        for (key, role, status) in [("student", "student", "active"), ("teacher", "teacher", "active"), ("admin", "admin", "active"), ("suspended", "student", "suspended")] {
            let u = insert_test_user(&conn, &format!("{key}@x.com"), role, status);
            tokens.insert(key, token_for(&conn, &u));
            ids.insert(key, u.id);
        }
        let state = Arc::new(AppState {
            config_store: exameow_core::config::ConfigStore::new("ExameowServerHttpTest").expect("a scratch config dir"),
            relay: crate::relay::init_db(":memory:").unwrap(),
            platform: PlatformState::for_tests(conn),
            admin_token: std::sync::Mutex::new("pass".into()),
            legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
            auth_failures: crate::token_limit::TokenLimiter::new(600),
        });
        // the neighbours the student routes share their paths with in `main.rs` (a static `bulk` next to `{subject_id}`)
        let app = routes()
            .route("/api/platform/enrollments", get(crate::platform_learning::my_enrollments_handler).post(crate::platform_learning::enroll_handler))
            .route("/api/platform/enrollments/{subject_id}", axum::routing::delete(crate::platform_learning::unenroll_handler))
            .route("/api/platform/assessments/available", get(crate::platform_exams::available_handler))
            .route("/api/platform/assessments/{id}", get(crate::platform_exams::detail_handler))
            .route("/api/platform/subjects/{id}/assessments", get(crate::platform_exams::subject_list_handler))
            .with_state(state.clone());
        H { app, state, tokens, ids }
    }

    async fn call(h: &H, method: Method, uri: &str, who: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
        if let Some(w) = who {
            b = b.header(header::AUTHORIZATION, format!("Bearer {}", h.tokens[w]));
        }
        let res = h.app.clone().oneshot(b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string()))).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn student_routes() -> Vec<(Method, &'static str, Option<Value>)> {
        vec![
            (Method::GET, "/api/platform/my/exams", None),
            (Method::GET, "/api/platform/student/overview", None),
            (Method::GET, "/api/platform/me/suggested-subjects", None),
            (Method::POST, "/api/platform/enrollments/bulk", Some(json!({"subject_ids": ["s1"]}))),
        ]
    }

    #[tokio::test]
    async fn only_an_active_student_reaches_the_student_routes() {
        let h = harness();
        for (m, uri, body) in student_routes() {
            for who in ["teacher", "admin", "suspended"] {
                let (st, v) = call(&h, m.clone(), uri, Some(who), body.clone()).await;
                assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{who} {uri}");
            }
            let (st, _) = call(&h, m.clone(), uri, None, body.clone()).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "no session: {uri}");
            let (st, v) = call(&h, m.clone(), uri, Some("student"), body.clone()).await;
            assert_eq!(st, StatusCode::OK, "student {uri}: {v}");
        }
    }

    #[tokio::test]
    async fn the_student_routes_answer_in_the_agreed_shapes() {
        let h = harness();
        let (_, v) = call(&h, Method::POST, "/api/platform/enrollments/bulk", Some("student"), Some(json!({"subject_ids": ["s1", "nope"]}))).await;
        assert_eq!(v, json!({"enrolled": 1, "skipped": 1}));
        let (_, v) = call(&h, Method::GET, "/api/platform/my/exams?tab=all&limit=5", Some("student"), None).await;
        assert_eq!(v, json!({"items": [], "total": 0, "counts": {"open": 0, "upcoming": 0, "in_progress": 0, "done": 0, "missed": 0}}));
        let (st, v) = call(&h, Method::GET, "/api/platform/my/exams?tab=bogus", Some("student"), None).await;
        assert_eq!((st, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_filter")));
        let (_, v) = call(&h, Method::GET, "/api/platform/student/overview", Some("student"), None).await;
        assert_eq!((v["enrolled"].clone(), v["checklist"].clone()), (json!(1), json!({"placement": false, "enrolled": true})));
        let (_, v) = call(&h, Method::GET, "/api/platform/me/suggested-subjects", Some("student"), None).await;
        assert_eq!(v, json!({"placement": null, "subjects": []}));
        for ids in [json!([]), json!((0..31).map(|i| format!("s{i}")).collect::<Vec<_>>())] {
            let (st, v) = call(&h, Method::POST, "/api/platform/enrollments/bulk", Some("student"), Some(json!({"subject_ids": ids}))).await;
            assert_eq!((st, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_selection")));
        }
    }

    #[tokio::test]
    async fn the_exam_page_carries_the_students_state_and_nobody_elses() {
        let h = harness();
        {
            let conn = h.state.platform.conn.lock().unwrap();
            conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at) VALUES (?1,'s1','approved',0)", params![h.ids["teacher"]]).unwrap();
            conn.execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,created_at,updated_at)
                 VALUES ('e1',?1,?1,'s1','امتحان','[]',1,1,'published',0,0), ('e2',?1,?1,'s1','قادم','[]',1,1,'published',0,0)",
                params![h.ids["teacher"]],
            )
            .unwrap();
            conn.execute("UPDATE assessments SET opens_at = ?1 WHERE id = 'e2'", params![now_ms() + 3_600_000]).unwrap();
        }
        // not enrolled yet: the page says why, and the old `can_start` agrees
        let (st, v) = call(&h, Method::GET, "/api/platform/assessments/e1", Some("student"), None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!((v["state"].clone(), v["reason"].clone(), v["phase"].clone(), v["can_start"].clone()), (json!("missed"), json!("not_enrolled"), json!("published"), json!(false)));
        call(&h, Method::POST, "/api/platform/enrollments/bulk", Some("student"), Some(json!({"subject_ids": ["s1"]}))).await;
        let (_, v) = call(&h, Method::GET, "/api/platform/assessments/e1", Some("student"), None).await;
        assert_eq!((v["state"].clone(), v["reason"].clone(), v["can_start"].clone()), (json!("open"), Value::Null, json!(true)));
        let (_, v) = call(&h, Method::GET, "/api/platform/assessments/e2", Some("student"), None).await;
        assert_eq!((v["state"].clone(), v["reason"].clone(), v["can_start"].clone()), (json!("upcoming"), json!("not_open"), json!(false)));
        // staff: the fields are there, and null
        for who in ["teacher", "admin"] {
            let (st, v) = call(&h, Method::GET, "/api/platform/assessments/e1", Some(who), None).await;
            assert_eq!(st, StatusCode::OK, "{who}");
            assert!(v["state"].is_null() && v["reason"].is_null() && v["phase"].is_null(), "{who}: {v}");
        }
        // the older "available" list keeps its array shape: open and upcoming here
        let (_, v) = call(&h, Method::GET, "/api/platform/assessments/available", Some("student"), None).await;
        let listed: Vec<&str> = v.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&"e1") && listed.contains(&"e2"));
    }

    /// A student never learns who owns or created an exam, nor how many attempts the whole class made: the fields stay
    /// in the JSON (same shape), their values are blanked; `attempt_count` is the student's own count.
    #[tokio::test]
    async fn an_exam_shown_to_a_student_hides_the_owner_and_the_class_attempt_count() {
        let h = harness();
        {
            let conn = h.state.platform.conn.lock().unwrap();
            conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at) VALUES (?1,'s1','approved',0)", params![h.ids["teacher"]]).unwrap();
            conn.execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,max_attempts,status,created_at,updated_at)
                 VALUES ('e1',?1,?1,'s1','امتحان','[]',1,1,3,'published',0,0)",
                params![h.ids["teacher"]],
            )
            .unwrap();
            // three attempts by classmates, one of mine
            for (i, who) in [("c1", "classmate-1"), ("c2", "classmate-2"), ("c3", "classmate-3")] {
                let u = insert_test_user(&conn, &format!("{who}@x.com"), "student", "active");
                conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status,score,pending) VALUES (?1,'e1',?2,1,'expired',0,0)", params![i, u.id]).unwrap();
            }
            conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status,score,pending) VALUES ('mine','e1',?1,1,'expired',0,0)", params![h.ids["student"]]).unwrap();
        }
        call(&h, Method::POST, "/api/platform/enrollments/bulk", Some("student"), Some(json!({"subject_ids": ["s1"]}))).await;
        let mut seen: Vec<(&str, Value)> = Vec::new();
        let (st, v) = call(&h, Method::GET, "/api/platform/assessments/e1", Some("student"), None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        seen.push(("detail", v));
        let (_, v) = call(&h, Method::GET, "/api/platform/assessments/available", Some("student"), None).await;
        // my only attempt used 1 of 3: still startable, so it is listed
        seen.push(("available", v[0].clone()));
        let (_, v) = call(&h, Method::GET, "/api/platform/subjects/s1/assessments", Some("student"), None).await;
        seen.push(("subject list", v[0].clone()));
        for (what, v) in &seen {
            assert_eq!(v["id"], json!("e1"), "{what}");
            assert_eq!((v["teacher_id"].clone(), v["created_by"].clone()), (Value::Null, Value::Null), "{what}: no owner ids");
            assert_eq!(v["attempt_count"], json!(1), "{what}: only my own attempt is counted, not the classmates' 3");
            assert_eq!(v["attempts_used"], json!(1), "{what}");
            assert_eq!(v["teacher_name"], json!("T"), "{what}: the name stays");
        }
        // staff keep the real values
        for who in ["teacher", "admin"] {
            let (_, v) = call(&h, Method::GET, "/api/platform/assessments/e1", Some(who), None).await;
            assert_eq!((v["teacher_id"].clone(), v["created_by"].clone(), v["attempt_count"].clone()), (json!(h.ids["teacher"]), json!(h.ids["teacher"]), json!(4)), "{who}");
        }
    }

    #[tokio::test]
    async fn bulk_lives_next_to_the_single_enrolment_routes() {
        let h = harness();
        let (st, v) = call(&h, Method::POST, "/api/platform/enrollments/bulk", Some("student"), Some(json!({"subject_ids": ["s1"]}))).await;
        assert_eq!((st, v), (StatusCode::OK, json!({"enrolled": 1, "skipped": 0})));
        let (_, v) = call(&h, Method::GET, "/api/platform/enrollments", Some("student"), None).await;
        assert_eq!((v[0]["subject_id"].clone(), v[0]["active"].clone()), (json!("s1"), json!(true)));
        let (st, _) = call(&h, Method::DELETE, "/api/platform/enrollments/s1", Some("student"), None).await;
        assert_eq!(st, StatusCode::NO_CONTENT, "{{subject_id}} still reaches the unenrol handler");
        let (_, v) = call(&h, Method::GET, "/api/platform/enrollments", Some("student"), None).await;
        assert_eq!(v, json!([]));
        let (st, _) = call(&h, Method::DELETE, "/api/platform/enrollments/bulk", Some("student"), None).await;
        assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED, "`bulk` is never taken for a subject id");
    }
}
