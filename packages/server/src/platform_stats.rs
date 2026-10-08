//! Platform statistics (phase 2-1): how many subjects, exam attempts and questions there are — per subject and per
//! institution for the admin cards, and platform-wide (cached) for the public landing page.
//!
//! Definitions (one place, so every screen agrees):
//! * **attempts** — submitted attempts only (`in_progress` / `expired` are not "an exam taken");
//! * **exams** — every exam of the subject, drafts included;
//! * **questions** — each distinct question once: the active question-bank items of the subject, plus the questions of
//!   its exams that are *not* backed by a still-active bank item of that subject (AI-generated, hand-written or imported
//!   straight into an exam, or copied from a bank item that was archived/deleted since). Exams are snapshots, so a bank
//!   item copied into several exams counts once, as the bank item.
//!
//! The three measures are independent CTEs: joining exams × attempts × bank in one pass would multiply `question_count`
//! by the number of attempts.

use crate::platform::{db_err, lock, require_admin, Res};
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// How long the public totals are reused (also the `max-age` the browser may cache them for).
const PUBLIC_TTL_MS: i64 = 60_000;

/// `?1` = institution id or NULL (all); `?2` = 1 for the public universe (active subjects of active institutions only).
const PER_SUBJECT_SQL: &str = "
WITH sub AS (
  SELECT s.id, s.institution_id, s.unit_id, s.is_active
  FROM subjects s JOIN institutions i ON i.id = s.institution_id
  WHERE (?1 IS NULL OR s.institution_id = ?1)
    AND (?2 = 0 OR (s.is_active = 1 AND i.is_active = 1))
), ex AS (
  SELECT a.subject_id AS sid, count(*) AS exams,
         SUM(MAX(0, a.question_count - (
           SELECT count(*)
           FROM json_each(CASE WHEN json_valid(a.source_map) THEN a.source_map ELSE '{}' END) j
           JOIN question_bank_items b ON b.id = j.value AND b.archived_at IS NULL AND b.subject_id = a.subject_id
         ))) AS own_q
  FROM assessments a JOIN sub ON sub.id = a.subject_id
  GROUP BY a.subject_id
), att AS (
  SELECT a.subject_id AS sid, count(*) AS n
  FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN sub ON sub.id = a.subject_id
  WHERE t.status = 'submitted'
  GROUP BY a.subject_id
), bk AS (
  SELECT b.subject_id AS sid, count(*) AS n
  FROM question_bank_items b JOIN sub ON sub.id = b.subject_id
  WHERE b.archived_at IS NULL
  GROUP BY b.subject_id
)
SELECT sub.id, sub.institution_id, sub.unit_id, sub.is_active,
       COALESCE(ex.exams, 0), COALESCE(att.n, 0), COALESCE(bk.n, 0) + COALESCE(ex.own_q, 0)
FROM sub LEFT JOIN ex ON ex.sid = sub.id LEFT JOIN att ON att.sid = sub.id LEFT JOIN bk ON bk.sid = sub.id
ORDER BY sub.id";

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SubjectStat {
    pub subject_id: String,
    pub institution_id: String,
    pub unit_id: Option<String>,
    pub is_active: bool,
    pub exams: i64,
    pub attempts: i64,
    pub questions: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct InstitutionStat {
    pub institution_id: String,
    pub subjects: i64,
    pub exams: i64,
    pub attempts: i64,
    pub questions: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Totals {
    pub questions: i64,
    pub subjects: i64,
    pub attempts: i64,
}

pub fn per_subject(conn: &Connection, institution: Option<&str>, public_only: bool) -> Res<Vec<SubjectStat>> {
    let mut stmt = conn.prepare(PER_SUBJECT_SQL).map_err(db_err)?;
    let rows = stmt
        .query_map(params![institution, public_only as i64], |r| {
            Ok(SubjectStat {
                subject_id: r.get(0)?,
                institution_id: r.get(1)?,
                unit_id: r.get(2)?,
                is_active: r.get(3)?,
                exams: r.get(4)?,
                attempts: r.get(5)?,
                questions: r.get(6)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// One row per institution that has at least one subject (the client reads a missing one as zeros); built from the
/// per-subject rows so an institution's figures always equal the sum of its subjects'.
pub fn per_institution(conn: &Connection) -> Res<Vec<InstitutionStat>> {
    let mut out: Vec<InstitutionStat> = Vec::new();
    for s in per_subject(conn, None, false)? {
        let row = match out.iter_mut().find(|i| i.institution_id == s.institution_id) {
            Some(r) => r,
            None => {
                out.push(InstitutionStat { institution_id: s.institution_id.clone(), ..Default::default() });
                out.last_mut().expect("just pushed")
            }
        };
        row.subjects += 1;
        row.exams += s.exams;
        row.attempts += s.attempts;
        row.questions += s.questions;
    }
    out.sort_by(|a, b| a.institution_id.cmp(&b.institution_id));
    Ok(out)
}

pub fn public_totals(conn: &Connection) -> Res<Totals> {
    let rows = per_subject(conn, None, true)?;
    Ok(Totals {
        subjects: rows.len() as i64,
        attempts: rows.iter().map(|r| r.attempts).sum(),
        questions: rows.iter().map(|r| r.questions).sum(),
    })
}

/// Short-lived memo of the public totals. The cache mutex is held while computing, so after expiry concurrent callers
/// share one computation (lock order: cache, then database — nothing takes them the other way round). Errors are never
/// cached; if a recompute fails the previous value is served rather than an error.
#[derive(Default)]
pub struct StatsCache(Mutex<Option<(i64, Totals)>>);

impl StatsCache {
    pub fn get_or_compute(&self, now: i64, ttl_ms: i64, compute: impl FnOnce() -> Res<Totals>) -> Res<Totals> {
        let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, totals)) = slot.as_ref() {
            if now >= *at && now - *at < ttl_ms {
                return Ok(totals.clone());
            }
        }
        match compute() {
            Ok(totals) => {
                *slot = Some((now, totals.clone()));
                Ok(totals)
            }
            Err(e) => match slot.as_ref() {
                Some((_, stale)) => Ok(stale.clone()),
                None => Err(e),
            },
        }
    }
}

// ───────── handlers ─────────

/// Public (no session): the three landing-page counters. Reachable through the `/api/platform/public/` prefix.
pub async fn public_stats_handler(State(s): State<Arc<AppState>>) -> Res<Response> {
    let totals = s.platform.public_stats.get_or_compute(now_ms(), PUBLIC_TTL_MS, || public_totals(&*lock(&s)?))?;
    Ok(([(header::CACHE_CONTROL, "public, max-age=60")], Json(totals)).into_response())
}

pub async fn institutions_stats_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<InstitutionStat>>> {
    require_admin(&s, &h)?;
    per_institution(&*lock(&s)?).map(Json)
}

#[derive(Deserialize)]
pub struct SubjectsQuery {
    institution_id: Option<String>,
}

pub async fn subjects_stats_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<SubjectsQuery>) -> Res<Json<Vec<SubjectStat>>> {
    require_admin(&s, &h)?;
    per_subject(&*lock(&s)?, q.institution_id.as_deref(), false).map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use crate::relay::err;
    use axum::http::StatusCode;

    fn world() -> Connection {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة صنعاء',1,0), ('i2','school','مدرسة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0), ('s2','i1','قواعد بيانات',1,0), ('s3','i2','رياضيات',1,0);",
        )
        .unwrap();
        conn
    }

    fn bank(conn: &Connection, id: &str, subject: &str, archived: bool) {
        conn.execute(
            "INSERT INTO question_bank_items(id,subject_id,type,stem,stem_norm,answer,created_at,updated_at,archived_at) VALUES (?1,?2,'short_answer','س','س','ج',0,0,?3)",
            params![id, subject, if archived { Some(1i64) } else { None }],
        )
        .unwrap();
    }

    fn exam(conn: &Connection, id: &str, subject: &str, count: i64, source_map: Option<&str>) {
        conn.execute(
            "INSERT INTO assessments(id,subject_id,title,questions,question_count,total_points,status,source_map,created_at,updated_at) VALUES (?1,?2,'t','[]',?3,?3,'published',?4,0,0)",
            params![id, subject, count, source_map],
        )
        .unwrap();
    }

    fn attempt(conn: &Connection, id: &str, exam: &str, student: &str, status: &str) {
        conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status) VALUES (?1,?2,?3,0,?4)", params![id, exam, student, status]).unwrap();
    }

    fn of(conn: &Connection, subject: &str) -> SubjectStat {
        per_subject(conn, None, false).unwrap().into_iter().find(|s| s.subject_id == subject).unwrap()
    }

    #[test]
    fn an_empty_platform_has_no_numbers() {
        let conn = create_test_db();
        assert_eq!(public_totals(&conn).unwrap(), Totals::default());
        assert!(per_institution(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_subject_without_anything_reads_zero_not_missing() {
        let s = of(&world(), "s1");
        assert_eq!((s.exams, s.attempts, s.questions), (0, 0, 0));
        assert_eq!((s.institution_id.as_str(), s.unit_id, s.is_active), ("i1", None, true));
    }

    #[test]
    fn archived_bank_items_are_not_counted() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        bank(&conn, "b2", "s1", false);
        bank(&conn, "b3", "s1", true);
        assert_eq!(of(&conn, "s1").questions, 2);
    }

    #[test]
    fn exam_questions_without_a_source_map_count_as_their_own() {
        let conn = world();
        exam(&conn, "e1", "s1", 5, None);
        assert_eq!(of(&conn, "s1").questions, 5);
        assert_eq!(of(&conn, "s1").exams, 1);
    }

    #[test]
    fn questions_copied_from_the_bank_are_not_counted_twice() {
        let conn = world();
        for b in ["b1", "b2", "b3"] {
            bank(&conn, b, "s1", false);
        }
        // 5 questions, 3 of them copied from bank items b1..b3 → 3 (bank) + 2 (exam only)
        exam(&conn, "e1", "s1", 5, Some(r#"{"q1":"b1","q2":"b2","q3":"b3"}"#));
        assert_eq!(of(&conn, "s1").questions, 5);
    }

    #[test]
    fn a_source_pointing_at_an_archived_or_missing_bank_item_counts_as_an_exam_question() {
        let conn = world();
        bank(&conn, "b1", "s1", true);
        exam(&conn, "e1", "s1", 3, Some(r#"{"q1":"b1","q2":"gone"}"#));
        assert_eq!(of(&conn, "s1").questions, 3);
    }

    #[test]
    fn a_bank_item_of_another_subject_does_not_hide_an_exam_question() {
        let conn = world();
        bank(&conn, "b9", "s2", false);
        exam(&conn, "e1", "s1", 2, Some(r#"{"q1":"b9"}"#));
        assert_eq!(of(&conn, "s1").questions, 2);
        assert_eq!(of(&conn, "s2").questions, 1);
    }

    #[test]
    fn a_malformed_source_map_does_not_fail_the_query() {
        let conn = world();
        exam(&conn, "e1", "s3", 8, Some("{not json"));
        exam(&conn, "e2", "s3", 0, Some(""));
        assert_eq!(of(&conn, "s3").questions, 8);
    }

    #[test]
    fn a_bank_item_reused_by_several_exams_counts_once() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        exam(&conn, "e1", "s1", 1, Some(r#"{"q1":"b1"}"#));
        exam(&conn, "e2", "s1", 1, Some(r#"{"q1":"b1"}"#));
        assert_eq!(of(&conn, "s1").questions, 1);
        assert_eq!(of(&conn, "s1").exams, 2);
    }

    #[test]
    fn an_exam_listing_more_sources_than_questions_never_goes_negative() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        bank(&conn, "b2", "s1", false);
        exam(&conn, "e1", "s1", 1, Some(r#"{"q1":"b1","q2":"b2"}"#));
        assert_eq!(of(&conn, "s1").questions, 2, "bank items 2 + max(0, 1 - 2)");
    }

    #[test]
    fn many_attempts_do_not_multiply_the_question_count() {
        let conn = world();
        exam(&conn, "e1", "s1", 5, None);
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        for i in 0..3 {
            attempt(&conn, &format!("a{i}"), "e1", &st.id, "submitted");
        }
        let s = of(&conn, "s1");
        assert_eq!((s.questions, s.attempts, s.exams), (5, 3, 1));
    }

    #[test]
    fn only_submitted_attempts_count() {
        let conn = world();
        exam(&conn, "e1", "s1", 1, None);
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        for (i, status) in ["submitted", "in_progress", "expired", "submitted"].iter().enumerate() {
            attempt(&conn, &format!("a{i}"), "e1", &st.id, status);
        }
        assert_eq!(of(&conn, "s1").attempts, 2);
    }

    #[test]
    fn institutions_are_isolated_and_the_filter_works() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        exam(&conn, "e3", "s3", 4, None);
        let only_i2 = per_subject(&conn, Some("i2"), false).unwrap();
        assert_eq!(only_i2.iter().map(|s| s.subject_id.as_str()).collect::<Vec<_>>(), vec!["s3"]);
        assert_eq!(only_i2[0].questions, 4);
        assert_eq!(of(&conn, "s1").questions, 1);
        assert!(per_subject(&conn, Some("nope"), false).unwrap().is_empty());
    }

    #[test]
    fn the_public_view_skips_inactive_subjects_and_institutions_but_the_admin_view_keeps_them() {
        let conn = world();
        exam(&conn, "e1", "s1", 3, None);
        exam(&conn, "e2", "s3", 7, None);
        conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i2'", []).unwrap();
        assert_eq!(public_totals(&conn).unwrap(), Totals { questions: 0, subjects: 1, attempts: 0 }, "only s2 is visible");
        assert_eq!(per_subject(&conn, None, false).unwrap().len(), 3);
        assert!(!of(&conn, "s1").is_active);
    }

    #[test]
    fn an_institutions_figures_equal_the_sum_of_its_subjects() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        exam(&conn, "e1", "s1", 3, None);
        exam(&conn, "e2", "s2", 2, None);
        exam(&conn, "e3", "s3", 9, None);
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        attempt(&conn, "a1", "e1", &st.id, "submitted");
        attempt(&conn, "a2", "e2", &st.id, "submitted");
        attempt(&conn, "a3", "e3", &st.id, "submitted");
        let rows = per_institution(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], InstitutionStat { institution_id: "i1".into(), subjects: 2, exams: 2, attempts: 2, questions: 6 });
        assert_eq!(rows[1], InstitutionStat { institution_id: "i2".into(), subjects: 1, exams: 1, attempts: 1, questions: 9 });
        assert_eq!(public_totals(&conn).unwrap(), Totals { questions: 15, subjects: 3, attempts: 3 });
    }

    #[test]
    fn deleting_a_subject_removes_its_numbers() {
        let conn = world();
        bank(&conn, "b1", "s1", false);
        exam(&conn, "e1", "s1", 3, None);
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        attempt(&conn, "a1", "e1", &st.id, "submitted");
        conn.execute("DELETE FROM subjects WHERE id = 's1'", []).unwrap();
        assert_eq!(public_totals(&conn).unwrap(), Totals { questions: 0, subjects: 2, attempts: 0 });
    }

    #[test]
    fn the_cache_computes_once_within_the_ttl_and_again_after_it() {
        let cache = StatsCache::default();
        let calls = std::cell::Cell::new(0);
        let compute = |n: i64| {
            calls.set(calls.get() + 1);
            Ok(Totals { questions: n, subjects: 0, attempts: 0 })
        };
        assert_eq!(cache.get_or_compute(1_000, 60_000, || compute(1)).unwrap().questions, 1);
        assert_eq!(cache.get_or_compute(30_000, 60_000, || compute(2)).unwrap().questions, 1, "still fresh");
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.get_or_compute(61_000, 60_000, || compute(3)).unwrap().questions, 3, "expired exactly at the ttl");
        assert_eq!(cache.get_or_compute(0, 60_000, || compute(4)).unwrap().questions, 4, "a clock that went backwards recomputes");
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn the_cache_never_stores_errors_and_serves_the_stale_value_when_a_recompute_fails() {
        let cache = StatsCache::default();
        let boom = || Err(err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"));
        assert!(cache.get_or_compute(0, 10, boom).is_err(), "nothing to fall back on");
        assert_eq!(cache.get_or_compute(1, 10, || Ok(Totals { questions: 7, subjects: 1, attempts: 2 })).unwrap().questions, 7, "the earlier error was not cached");
        assert_eq!(cache.get_or_compute(100, 10, boom).unwrap().questions, 7, "expired, recompute fails → stale value");
    }
}
