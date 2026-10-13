//! The student's own statistics (phase 4-3): `GET /api/platform/my/stats`.
//!
//! Private to the student: everything is scoped by the authenticated student's id (read from the session, never from the
//! request) and computed on demand - there is no shared cache, no class average, no other student's number and **no
//! ranking** anywhere in it.
//!
//! **What counts.** An attempt is counted only when it is the student's own, `submitted`, **released** (`platform_exams::
//! is_released`: a result the student is not yet allowed to read never shows up here either), **fully graded** (no written
//! answer still waiting, `pending = 0`) and belongs to an exam worth points (a percentage needs a total). Exams are counted
//! whatever their visibility today - the results are the student's own. Averages use the student's **best attempt per exam**
//! (`platform_exams::best_graded_attempt`'s rule: the highest score, the newest on a tie), so a retake can only help. The
//! trend lists attempts, not exams: the last [`TREND_LEN`] counted attempts of the subject, oldest first. Averages are taken
//! over the exact percentages and rounded once (two decimals).
//!
//! **Weakest chapters.** Taken from the best attempt of each exam: every ANSWERED question (something was written or chosen)
//! of a chapter adds its points and its maximum; a question worth 0 points (voided by an answer-key correction) is skipped.
//! A chapter is listed when it has at least [`WEAK_MIN_ANSWERED`] answered questions and its percentage is below
//! [`WEAK_BELOW_PERCENT`]; the lowest first, at most [`WEAK_MAX`] per subject.

use crate::platform::{db_err, lock, require_role, Res, User};
use crate::platform_exams::{is_released, round2, Outcome};
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use exameow_core::exam::Question;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Attempts in a subject's trend.
pub(crate) const TREND_LEN: usize = 8;
/// A chapter needs at least this many answered questions before it is judged.
pub(crate) const WEAK_MIN_ANSWERED: i64 = 3;
/// A chapter is "weak" below this percentage.
pub(crate) const WEAK_BELOW_PERCENT: f64 = 70.0;
/// Weak chapters listed per subject.
pub(crate) const WEAK_MAX: usize = 3;
/// The weak-chapter analysis reads the questions of the exams behind the student's most recent best attempts only, this many
/// in all: each one costs a query and parsing the exam's whole question JSON, under the one database lock, so a student with
/// hundreds of exams must not turn a page load into hundreds of them. Counts, averages and trends are over everything.
pub(crate) const WEAK_ANALYSIS_MAX: usize = 60;
/// `GET /my/stats` requests per student per minute (a page asks once; this stops a reload loop, not a student).
const STATS_PER_MINUTE: i64 = 30;

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct Overall {
    /// Counted attempts (see the module docs).
    pub attempts: i64,
    /// Exams with at least one counted attempt.
    pub exams: i64,
    /// Mean of the best attempt's percentage per exam, two decimals; `null` without any exam.
    pub average_percent: Option<f64>,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct TrendPoint {
    pub attempt_id: String,
    pub exam_id: String,
    pub exam_title: String,
    pub percent: f64,
    pub submitted_at: i64,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct WeakChapter {
    pub chapter: String,
    pub percent: f64,
    /// Answered questions of the chapter that the percentage is over.
    pub answered: i64,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct SubjectStats {
    pub subject_id: String,
    pub subject_name: String,
    pub exams: i64,
    pub attempts: i64,
    pub average_percent: Option<f64>,
    /// The last [`TREND_LEN`] counted attempts, oldest first.
    pub trend: Vec<TrendPoint>,
    pub weakest: Vec<WeakChapter>,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct MyStats {
    pub overall: Overall,
    pub subjects: Vec<SubjectStats>,
}

/// One counted attempt.
struct Counted {
    attempt_id: String,
    exam_id: String,
    exam_title: String,
    subject_id: String,
    subject_name: String,
    score: f64,
    total: f64,
    stamp: i64,
}

impl Counted {
    /// The exact percentage; averages are taken over these and rounded once, at the end.
    fn exact_percent(&self) -> f64 {
        self.score / self.total * 100.0
    }

    fn percent(&self) -> f64 {
        round2(self.exact_percent())
    }
}

/// The student's counted attempts (released, fully graded, submitted, on an exam with points).
fn counted_attempts(conn: &Connection, student_id: &str, now: i64) -> Res<Vec<Counted>> {
    let rows = conn
        .prepare(
            "SELECT t.id, a.id, a.title, a.subject_id, s.name_ar, t.score, a.total_points, COALESCE(t.submitted_at, t.started_at), a.release_mode, a.status, a.closes_at
               FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN subjects s ON s.id = a.subject_id
              WHERE t.student_id = ?1 AND t.status = 'submitted' AND t.pending = 0 AND a.total_points > 0",
        )
        .map_err(db_err)?
        .query_map(params![student_id], |r| {
            let released = is_released(&r.get::<_, String>(8)?, &r.get::<_, String>(9)?, r.get(10)?, now);
            Ok((
                released,
                Counted {
                    attempt_id: r.get(0)?,
                    exam_id: r.get(1)?,
                    exam_title: r.get(2)?,
                    subject_id: r.get(3)?,
                    subject_name: r.get(4)?,
                    score: r.get(5)?,
                    total: r.get(6)?,
                    stamp: r.get(7)?,
                },
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows.into_iter().filter(|(released, _)| *released).map(|(_, c)| c).collect())
}

/// The best attempt of each exam among `attempts`: the highest score, the newest on a tie (hand-in time, then id) - the same
/// order as `platform_exams::best_graded_attempt`.
fn best_per_exam(attempts: &[Counted]) -> HashMap<&str, &Counted> {
    let mut best: HashMap<&str, &Counted> = HashMap::new();
    for a in attempts {
        let better = best.get(a.exam_id.as_str()).map_or(true, |b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.stamp.cmp(&b.stamp))
                .then_with(|| a.attempt_id.cmp(&b.attempt_id))
                .is_gt()
        });
        if better {
            best.insert(a.exam_id.as_str(), a);
        }
    }
    best
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| round2(values.iter().sum::<f64>() / values.len() as f64))
}

/// `(points earned, points available, answered questions)` per chapter of ONE attempt: answered questions only, voided
/// ones (worth 0) skipped. Questions without a chapter belong to no chapter.
fn chapter_marks(conn: &Connection, attempt_id: &str) -> Res<BTreeMap<String, (f64, f64, i64)>> {
    let (questions_raw, answers_raw, results_raw): (String, String, String) = conn
        .query_row(
            "SELECT a.questions, t.answers, t.results FROM attempts t JOIN assessments a ON a.id = t.assessment_id WHERE t.id = ?1",
            params![attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db_err)?;
    let questions: Vec<Question> = serde_json::from_str(&questions_raw).unwrap_or_default();
    let answers: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
    let outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).unwrap_or_default();
    let by_id: HashMap<&str, &Outcome> = outcomes.iter().map(|o| (o.id.as_str(), o)).collect();
    let mut out: BTreeMap<String, (f64, f64, i64)> = BTreeMap::new();
    for q in &questions {
        let Some(chapter) = q.chapter.as_deref().map(str::trim).filter(|c| !c.is_empty()) else { continue };
        let Some(o) = by_id.get(q.id.as_str()) else { continue };
        let answered = answers.get(&q.id).map_or(false, |a| !a.trim().is_empty());
        if o.max <= 0.0 || !answered {
            continue;
        }
        let e = out.entry(chapter.to_string()).or_insert((0.0, 0.0, 0));
        e.0 += o.points;
        e.1 += o.max;
        e.2 += 1;
    }
    Ok(out)
}

/// The weakest chapters of one subject from the best attempt of each of its exams.
fn weakest_chapters(conn: &Connection, best: &[&Counted]) -> Res<Vec<WeakChapter>> {
    let mut sums: BTreeMap<String, (f64, f64, i64)> = BTreeMap::new();
    for b in best {
        for (chapter, (earned, available, answered)) in chapter_marks(conn, &b.attempt_id)? {
            let e = sums.entry(chapter).or_insert((0.0, 0.0, 0));
            e.0 += earned;
            e.1 += available;
            e.2 += answered;
        }
    }
    let mut weak: Vec<WeakChapter> = sums
        .into_iter()
        .filter(|(_, (_, available, answered))| *answered >= WEAK_MIN_ANSWERED && *available > 0.0)
        .map(|(chapter, (earned, available, answered))| WeakChapter { chapter, percent: round2(earned / available * 100.0), answered })
        .filter(|w| w.percent < WEAK_BELOW_PERCENT)
        .collect();
    weak.sort_by(|a, b| a.percent.partial_cmp(&b.percent).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.answered.cmp(&a.answered)).then_with(|| a.chapter.cmp(&b.chapter)));
    weak.truncate(WEAK_MAX);
    Ok(weak)
}

pub(crate) fn my_stats(conn: &Connection, student: &User, now: i64) -> Res<MyStats> {
    let counted = counted_attempts(conn, &student.id, now)?;
    let best = best_per_exam(&counted);
    let best_percents: Vec<f64> = best.values().map(|b| b.exact_percent()).collect();
    let overall = Overall { attempts: counted.len() as i64, exams: best.len() as i64, average_percent: mean(&best_percents) };

    let mut subject_ids: Vec<(&str, &str)> = counted.iter().map(|c| (c.subject_id.as_str(), c.subject_name.as_str())).collect::<HashSet<_>>().into_iter().collect();
    subject_ids.sort_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)));
    // the attempts whose questions the weak-chapter analysis may read: the newest best attempts, capped
    let mut newest: Vec<&Counted> = best.values().copied().collect();
    newest.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| b.attempt_id.cmp(&a.attempt_id)));
    let analysed: HashSet<&str> = newest.iter().take(WEAK_ANALYSIS_MAX).map(|c| c.attempt_id.as_str()).collect();
    let mut subjects = Vec::with_capacity(subject_ids.len());
    for (subject_id, subject_name) in subject_ids {
        let mine: Vec<&Counted> = counted.iter().filter(|c| c.subject_id == subject_id).collect();
        let best_here: Vec<&Counted> = best.values().copied().filter(|b| b.subject_id == subject_id).collect();
        let percents: Vec<f64> = best_here.iter().map(|b| b.exact_percent()).collect();
        let mut by_time = mine.clone();
        by_time.sort_by(|a, b| a.stamp.cmp(&b.stamp).then_with(|| a.attempt_id.cmp(&b.attempt_id)));
        let trend = by_time
            .iter()
            .skip(by_time.len().saturating_sub(TREND_LEN))
            .map(|c| TrendPoint { attempt_id: c.attempt_id.clone(), exam_id: c.exam_id.clone(), exam_title: c.exam_title.clone(), percent: c.percent(), submitted_at: c.stamp })
            .collect();
        subjects.push(SubjectStats {
            subject_id: subject_id.to_string(),
            subject_name: subject_name.to_string(),
            exams: best_here.len() as i64,
            attempts: mine.len() as i64,
            average_percent: mean(&percents),
            trend,
            weakest: weakest_chapters(conn, &best_here.iter().copied().filter(|b| analysed.contains(b.attempt_id.as_str())).collect::<Vec<_>>())?,
        });
    }
    Ok(MyStats { overall, subjects })
}

pub async fn stats_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<MyStats>> {
    let student = require_role(&s, &h, "student")?;
    let conn = lock(&s)?;
    crate::platform::rate_limit(&conn, &format!("stats:{}", student.id), 60_000, STATS_PER_MINUTE)?;
    my_stats(&conn, &student, now_ms()).map(Json)
}

/// The statistics route (merged into the app in `main.rs`; private by default through the auth gate).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/platform/my/stats", get(stats_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_exam_key::{correct_question, CorrectReq};
    use crate::platform_exams::fixtures::*;
    use crate::platform_exams::{grade_attempt, start_attempt, GradeReq, GRACE_MS};
    use serde_json::{json, Value};

    /// `n` single-choice questions (right answer B) of one chapter, ids `<prefix><i>`.
    fn chapter(name: &str, prefix: &str, n: usize) -> Vec<Value> {
        (1..=n)
            .map(|i| json!({"id": format!("{prefix}{i}"), "type": "single_choice", "stem": "س", "options": ["أ", "ب", "ج"], "answer": "B", "score": 1, "chapter": name}))
            .collect()
    }

    /// An answer sheet: the first `right` questions of `prefix` right, the next `wrong` wrong, the rest left blank.
    fn sheet(prefix: &str, right: usize, wrong: usize) -> Vec<(String, String)> {
        let mut out = vec![];
        for i in 1..=right {
            out.push((format!("{prefix}{i}"), "B".to_string()));
        }
        for i in right + 1..=right + wrong {
            out.push((format!("{prefix}{i}"), "A".to_string()));
        }
        out
    }

    fn map(pairs: Vec<(String, String)>) -> HashMap<String, String> {
        pairs.into_iter().collect()
    }

    fn stats(w: &W, now: i64) -> MyStats {
        my_stats(&w.conn, &w.student, now).unwrap()
    }

    #[test]
    fn the_weak_chapter_analysis_reads_only_the_newest_best_attempts_but_the_counts_cover_everything() {
        let w = world();
        // the oldest exam: a chapter the student failed completely
        let old = exam(&w, json!({"title": "قديم", "questions": chapter("قديم", "o", 3)}));
        take(&w, &w.student, &old, NOW, MIN, &map(sheet("o", 0, 3)));
        // WEAK_ANALYSIS_MAX - 1 newer exams, all passed: the oldest is still inside the cap
        for i in 0..WEAK_ANALYSIS_MAX - 1 {
            let id = exam(&w, json!({"title": format!("جديد {i}"), "questions": chapter("جديد", "n", 1)}));
            take(&w, &w.student, &id, NOW + (i as i64 + 1) * H, MIN, &map(sheet("n", 1, 0)));
        }
        let later = NOW + 1_000 * H;
        let s = stats(&w, later);
        assert_eq!(s.overall.exams, WEAK_ANALYSIS_MAX as i64);
        assert_eq!(s.subjects[0].weakest.iter().map(|c| c.chapter.as_str()).collect::<Vec<_>>(), ["قديم"], "inside the cap: the weak chapter is found");
        // one more newer exam pushes it beyond the cap: it is no longer read - the counts still include it
        let id = exam(&w, json!({"title": "الأحدث", "questions": chapter("جديد", "n", 1)}));
        take(&w, &w.student, &id, NOW + 500 * H, MIN, &map(sheet("n", 1, 0)));
        let s = stats(&w, later);
        assert!(s.subjects[0].weakest.is_empty(), "beyond the cap: only the newest best attempts are analysed");
        assert_eq!((s.overall.exams, s.overall.attempts, s.subjects[0].exams), (WEAK_ANALYSIS_MAX as i64 + 1, WEAK_ANALYSIS_MAX as i64 + 1, WEAK_ANALYSIS_MAX as i64 + 1));
        let expected_average = round2(((WEAK_ANALYSIS_MAX as f64) * 100.0) / (WEAK_ANALYSIS_MAX as f64 + 1.0));
        assert_eq!(s.overall.average_percent, Some(expected_average), "the average still covers the old exam");
    }

    #[test]
    fn a_student_with_no_counted_attempts_has_an_empty_page() {
        let w = world();
        assert_eq!(stats(&w, NOW), MyStats { overall: Overall { attempts: 0, exams: 0, average_percent: None }, subjects: vec![] });
        // a running attempt and an expired one are not results
        let id = exam(&w, json!({}));
        start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
        assert_eq!(stats(&w, NOW + 1).overall.attempts, 0);
    }

    #[test]
    fn the_average_uses_the_best_attempt_of_each_exam_and_counts_every_attempt() {
        let w = world();
        let a = exam(&w, json!({"title": "أ", "max_attempts": 3}));
        let b = exam(&w, json!({"title": "ب"}));
        take(&w, &w.student, &a, NOW, MIN, &objective_right()); // 5 of 9 = 55.56
        let best = take(&w, &w.student, &a, NOW + 10 * MIN, MIN, &all_right()); // + the written answer
        take(&w, &w.student, &b, NOW + 20 * MIN, MIN, &ans(&[("q1", "B"), ("q2", "AC")])); // 3 of 9 = 33.33
        // the written answer of the best attempt still waits for a grade: that attempt does not count yet
        let before = stats(&w, NOW + H);
        assert_eq!((before.overall.attempts, before.overall.exams), (2, 2), "the ungraded attempt is left out");
        assert_eq!(before.overall.average_percent, Some(round2((5.0 / 9.0 * 100.0 + 3.0 / 9.0 * 100.0) / 2.0)));
        grade_attempt(&w.conn, &w.admin, &best, &GradeReq { grades: [("q5".to_string(), 4.0)].into(), ..Default::default() }).unwrap();
        let after = stats(&w, NOW + H);
        assert_eq!((after.overall.attempts, after.overall.exams), (3, 2));
        assert_eq!(after.overall.average_percent, Some(round2((100.0 + 3.0 / 9.0 * 100.0) / 2.0)), "the best of exam A (9 of 9), not the mean of its two attempts");
        let s = &after.subjects[0];
        assert_eq!((s.subject_id.as_str(), s.subject_name.as_str(), s.exams, s.attempts), ("s1", "برمجة", 2, 3));
        assert_eq!(s.average_percent, after.overall.average_percent);
    }

    #[test]
    fn a_result_the_student_may_not_read_yet_is_not_counted() {
        let w = world();
        let c = NOW + H;
        let id = exam(&w, json!({"release_mode": "after_close", "closes_at": c}));
        take(&w, &w.student, &id, NOW, MIN, &objective_right());
        assert_eq!(stats(&w, c + GRACE_MS).overall.attempts, 0, "withheld until the last attempt could have landed");
        let after = stats(&w, c + GRACE_MS + 1);
        assert_eq!((after.overall.attempts, after.overall.exams), (1, 1));
        // closed by hand releases it at once
        let hand = exam(&w, json!({"title": "يدوي", "release_mode": "after_close", "closes_at": NOW + 10 * H}));
        take(&w, &w.student, &hand, NOW, MIN, &objective_right());
        assert_eq!(stats(&w, NOW + 5 * MIN).overall.exams, 0, "both are still withheld");
        crate::platform_exam_admin::act(&w.conn, &w.admin, &hand, "close", NOW + 6 * MIN).unwrap();
        assert_eq!(stats(&w, NOW + 7 * MIN).overall.exams, 1, "the one that was closed by hand counts at once; the timed one still waits");
    }

    #[test]
    fn only_the_students_own_attempts_count_and_the_page_has_no_ranking_or_class_numbers() {
        let w = world();
        let id = exam(&w, json!({}));
        let other = enroll(&w, "o@x.com");
        let theirs_at = take(&w, &other, &id, NOW, MIN, &all_right());
        grade_attempt(&w.conn, &w.admin, &theirs_at, &GradeReq { grades: [("q5".to_string(), 4.0)].into(), ..Default::default() }).unwrap(); // 9 of 9
        assert_eq!(stats(&w, NOW + H).overall.attempts, 0, "a classmate's attempt is nobody's business");
        take(&w, &w.student, &id, NOW + 5 * MIN, MIN, &objective_right());
        let mine = stats(&w, NOW + H);
        assert_eq!((mine.overall.attempts, mine.overall.average_percent), (1, Some(round2(5.0 / 9.0 * 100.0))), "the classmate's 100 % does not move my average");
        let json = serde_json::to_value(&mine).unwrap();
        let keys = |v: &Value| -> Vec<String> {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&json), ["overall", "subjects"]);
        assert_eq!(keys(&json["overall"]), ["attempts", "average_percent", "exams"]);
        assert_eq!(keys(&json["subjects"][0]), ["attempts", "average_percent", "exams", "subject_id", "subject_name", "trend", "weakest"]);
        assert_eq!(keys(&json["subjects"][0]["trend"][0]), ["attempt_id", "exam_id", "exam_title", "percent", "submitted_at"]);
        // the other student sees their own page, with their own numbers - and none of mine
        let theirs = my_stats(&w.conn, &other, NOW + H).unwrap();
        assert_eq!((theirs.overall.attempts, theirs.overall.average_percent), (1, Some(100.0)));
        assert_eq!(theirs.subjects[0].trend.len(), 1);
    }

    #[test]
    fn subjects_are_separate_and_a_hidden_exam_still_counts_for_its_own_student() {
        let w = world();
        enroll_in(&w, "s2");
        let a = exam(&w, json!({"title": "برمجة"}));
        let b = exam(&w, json!({"title": "رياضيات", "subject_id": "s2"}));
        take(&w, &w.student, &a, NOW, MIN, &objective_right());
        take(&w, &w.student, &b, NOW + 5 * MIN, MIN, &ans(&[("q1", "B")]));
        let st = stats(&w, NOW + H);
        assert_eq!(st.subjects.iter().map(|s| (s.subject_name.as_str(), s.exams, s.attempts)).collect::<Vec<_>>(), [("برمجة", 1, 1), ("رياضيات", 1, 1)]);
        assert_eq!(st.overall.exams, 2);
        assert_eq!(st.overall.average_percent, Some(round2((5.0 / 9.0 * 100.0 + 1.0 / 9.0 * 100.0) / 2.0)));
        // the results are the student's own, whatever happens to the subject afterwards
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(stats(&w, NOW + H).overall.attempts, 2);
    }

    fn enroll_in(w: &W, subject: &str) {
        w.conn.execute("INSERT OR IGNORE INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, ?2, 0)", params![w.student.id, subject]).unwrap();
    }

    #[test]
    fn the_trend_is_the_last_eight_attempts_oldest_first() {
        let w = world();
        let id = exam(&w, json!({"max_attempts": 10}));
        let other = exam(&w, json!({"title": "آخر"}));
        let mut ids = vec![];
        for i in 0..10 {
            // alternate between fully right and fully wrong objective sheets, one attempt every ten minutes
            let sheet = if i % 2 == 0 { objective_right() } else { ans(&[("q1", "A")]) };
            ids.push(take(&w, &w.student, &id, NOW + i * 10 * MIN, MIN, &sheet));
        }
        let o = take(&w, &w.student, &other, NOW + 7 * MIN, MIN, &objective_right()); // between the first attempts
        let s = &stats(&w, NOW + 5 * H).subjects[0];
        assert_eq!(s.attempts, 11);
        assert_eq!(s.trend.len(), 8);
        let times: Vec<i64> = s.trend.iter().map(|t| t.submitted_at).collect();
        assert!(times.windows(2).all(|p| p[0] < p[1]), "oldest first: {times:?}");
        // the three oldest (attempt 0 at +1 min, the other exam at +8 min, attempt 1 at +11 min) are cut
        let got: Vec<&str> = s.trend.iter().map(|t| t.attempt_id.as_str()).collect();
        let want: Vec<&str> = ids[2..].iter().map(String::as_str).collect();
        assert_eq!(got, want);
        assert!(!got.contains(&o.as_str()));
        assert_eq!((s.trend[0].percent, s.trend[1].percent), (round2(5.0 / 9.0 * 100.0), 0.0), "a percentage per attempt");
        assert_eq!((s.trend[0].exam_id.as_str(), s.trend[0].exam_title.as_str()), (id.as_str(), "امتحان"));
    }

    #[test]
    fn the_weakest_chapters_need_three_answers_and_fall_below_seventy_percent_lowest_first_at_most_three() {
        let w = world();
        let mut qs = vec![];
        let mut answers: Vec<(String, String)> = vec![];
        // (chapter, prefix, questions, right, wrong); the rest of the questions are left blank
        for (name, prefix, n, right, wrong) in [
            ("ضعيف أ", "a", 4, 1, 3),   // 25 %
            ("ضعيف ب", "b", 3, 1, 2),   // 33.33 %
            ("ضعيف ج", "c", 3, 2, 1),   // 66.67 %
            ("ضعيف د", "d", 3, 0, 3),   // 0 %
            ("قوي", "e", 3, 3, 0),      // 100 %
            ("قليل", "f", 2, 0, 2),     // 0 % but only two answers
            ("تخطي", "g", 5, 0, 2),     // two answers, three skipped
        ] {
            qs.extend(chapter(name, prefix, n));
            answers.extend(sheet(prefix, right, wrong));
        }
        qs.push(json!({"id": "x1", "type": "single_choice", "stem": "س", "options": ["أ", "ب"], "answer": "B", "score": 1})); // no chapter
        answers.push(("x1".into(), "A".into()));
        let id = exam(&w, json!({"questions": qs}));
        take(&w, &w.student, &id, NOW, MIN, &map(answers));
        let weak = &stats(&w, NOW + H).subjects[0].weakest;
        assert_eq!(
            weak,
            &vec![
                WeakChapter { chapter: "ضعيف د".into(), percent: 0.0, answered: 3 },
                WeakChapter { chapter: "ضعيف أ".into(), percent: 25.0, answered: 4 },
                WeakChapter { chapter: "ضعيف ب".into(), percent: 33.33, answered: 3 },
            ],
            "the fourth weak one (66.67 %) is cut; 100 % is strong; two answers are too few; skipped questions are not answers"
        );
    }

    #[test]
    fn seventy_percent_exactly_is_not_weak_and_the_best_attempt_of_each_exam_feeds_the_chapters() {
        let w = world();
        let id = exam(&w, json!({"questions": chapter("حد", "h", 10), "max_attempts": 3}));
        take(&w, &w.student, &id, NOW, MIN, &map(sheet("h", 7, 3)));
        assert!(stats(&w, NOW + H).subjects[0].weakest.is_empty(), "7 of 10 = 70 %: not below 70");
        let id2 = exam(&w, json!({"title": "ثان", "questions": chapter("حد", "k", 10), "max_attempts": 3}));
        let bad = take(&w, &w.student, &id2, NOW + 5 * MIN, MIN, &map(sheet("k", 3, 7)));
        assert_eq!(stats(&w, NOW + H).subjects[0].weakest, vec![WeakChapter { chapter: "حد".into(), percent: round2(10.0 / 20.0 * 100.0), answered: 20 }], "(7 + 3) of 20 across both exams");
        // a better second attempt of the second exam replaces the worse one in the chapter figures (best per exam)
        let good = take(&w, &w.student, &id2, NOW + 10 * MIN, MIN, &map(sheet("k", 9, 1)));
        assert_ne!(bad, good);
        assert!(stats(&w, NOW + H).subjects[0].weakest.is_empty(), "(7 + 9) of 20 = 80 %: the weak attempt no longer counts");
    }

    #[test]
    fn a_voided_question_is_skipped_everywhere() {
        let w = world();
        let id = exam(&w, json!({"questions": chapter("فصل", "v", 4)}));
        take(&w, &w.student, &id, NOW, MIN, &map(sheet("v", 0, 4))); // 4 wrong: 0 %
        assert_eq!(stats(&w, NOW + H).subjects[0].weakest, vec![WeakChapter { chapter: "فصل".into(), percent: 0.0, answered: 4 }]);
        let void: CorrectReq = serde_json::from_value(json!({"mode": "void"})).unwrap();
        correct_question(&w.conn, &w.admin, &id, "v1", &void, NOW + 2 * MIN).unwrap();
        // three answered questions are left: still judged; one more voided and the chapter is no longer judged at all
        assert_eq!(stats(&w, NOW + H).subjects[0].weakest, vec![WeakChapter { chapter: "فصل".into(), percent: 0.0, answered: 3 }]);
        correct_question(&w.conn, &w.admin, &id, "v2", &void, NOW + 3 * MIN).unwrap();
        assert!(stats(&w, NOW + H).subjects[0].weakest.is_empty());
        // an exam where everything is voided is worth 0 points: it has no percentage, so it is left out altogether
        for q in ["v3", "v4"] {
            correct_question(&w.conn, &w.admin, &id, q, &void, NOW + 4 * MIN).unwrap();
        }
        assert_eq!(stats(&w, NOW + H), MyStats { overall: Overall { attempts: 0, exams: 0, average_percent: None }, subjects: vec![] });
    }
}
