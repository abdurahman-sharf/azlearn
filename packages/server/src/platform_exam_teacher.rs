//! The teacher's door to the exam builder (phase 3-3): `/api/platform/teacher/exams*`.
//!
//! There is no second implementation: every handler below calls the engine in `platform_exam_admin` with the signed-in
//! teacher, and the engine's actor-aware rules do the rest (ownership, the freeze, the `locked` rule, the lifecycle).
//! What lives here is only what is specific to a teacher at the HTTP edge:
//!
//! * the account must be an **active teacher** (`require_role`), and the exam's owner is always read from the database,
//!   never from the request — another teacher's exam answers 403 `forbidden`, a missing one 404;
//! * the `teachers.can_create_exams` switch ([`ensure_exams_enabled`]) gates creating an exam, publishing one and
//!   duplicating one — and nothing else, so an exam that is already live stays fully manageable with the switch off
//!   (edit, close, reopen, archive, grade, correct the key);
//! * deleting an exam that has attempts is refused (409 `has_attempts`): there is no typed-title override for teachers.
//!
//! [`routes`] registers the admin and teacher exam routes together (and the answer-key correction of both), so that a
//! path conflict between `/{id}/{action}` and `/{id}/questions/{qid}/correct` is caught by a test that builds the router.

use crate::platform::{lock, require_role, Res, User};
use crate::platform_exam_admin::{self as engine, ExamDetail, ExamPage, ExamReq, ListQuery};
use crate::platform_exam_key;
use crate::platform_exams::{ensure_exams_enabled, get_info};
use crate::platform_settings::Crypto;
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use rusqlite::Connection;
use std::sync::Arc;

/// Creating an exam, as a teacher: the "teachers may create exams" switch first, then the engine (assignment, then
/// the same validation as the admin builder).
pub fn create_checked(conn: &Connection, crypto: &Crypto, teacher: &User, r: &ExamReq, now: i64) -> Res<ExamDetail> {
    ensure_exams_enabled(conn, crypto)?;
    engine::create_exam(conn, teacher, r, now)
}

/// A lifecycle action, as a teacher. Publishing and duplicating create *new* offers to students, so they need the
/// switch; the owner is looked up first so that someone else's exam still answers 403 `forbidden` rather than a
/// hint about the switch. Reopening, closing and archiving never need it.
pub fn lifecycle_checked(conn: &Connection, crypto: &Crypto, teacher: &User, id: &str, action: &str, now: i64) -> Res<(StatusCode, ExamDetail)> {
    if matches!(action, "publish" | "duplicate") && get_info(conn, &teacher.id, id)?.owned_by(teacher) {
        ensure_exams_enabled(conn, crypto)?;
    }
    engine::lifecycle(conn, teacher, id, action, now)
}

// ───────── handlers ─────────

async fn list_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(f): Query<ListQuery>) -> Res<Json<ExamPage>> {
    let t = require_role(&s, &h, "teacher")?;
    engine::list_exams(&*lock(&s)?, &t, &f, now_ms()).map(Json)
}

async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<ExamReq>) -> Res<(StatusCode, Json<ExamDetail>)> {
    let t = require_role(&s, &h, "teacher")?;
    create_checked(&*lock(&s)?, &s.platform.crypto, &t, &r, now_ms()).map(|d| (StatusCode::CREATED, Json(d)))
}

async fn get_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<ExamDetail>> {
    let t = require_role(&s, &h, "teacher")?;
    engine::get_exam(&*lock(&s)?, &t, &id, now_ms()).map(Json)
}

async fn update_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<ExamReq>) -> Res<Json<ExamDetail>> {
    let t = require_role(&s, &h, "teacher")?;
    engine::update_exam(&*lock(&s)?, &t, &id, &r, now_ms()).map(Json)
}

async fn delete_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let t = require_role(&s, &h, "teacher")?;
    // no `confirm_title` for a teacher: an exam somebody sat is never deleted, only closed or archived
    engine::delete_exam(&*lock(&s)?, &t, &id, None)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn action_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path((id, action)): Path<(String, String)>) -> Res<(StatusCode, Json<ExamDetail>)> {
    let t = require_role(&s, &h, "teacher")?;
    lifecycle_checked(&*lock(&s)?, &s.platform.crypto, &t, &id, &action, now_ms()).map(|(code, d)| (code, Json(d)))
}

/// Every exam-builder route, admin and teacher: the CRUD, the lifecycle actions and the answer-key correction. Built in
/// one place on purpose — `/{id}/{action}` and `/{id}/questions/{qid}/correct` share a prefix, and `Router::route` panics
/// on a conflicting registration, which `routes_register_without_conflicts` turns into a test failure.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/platform/admin/exams", get(engine::list_handler).post(engine::create_handler))
        .route("/api/platform/admin/exams/{id}", get(engine::get_handler).patch(engine::update_handler).delete(engine::delete_handler))
        .route("/api/platform/admin/exams/{id}/{action}", post(engine::action_handler))
        .route("/api/platform/admin/exams/{id}/questions/{qid}/correct", post(platform_exam_key::admin_handler))
        .route("/api/platform/teacher/exams", get(list_handler).post(create_handler))
        .route("/api/platform/teacher/exams/{id}", get(get_handler).patch(update_handler).delete(delete_handler))
        .route("/api/platform/teacher/exams/{id}/{action}", post(action_handler))
        .route("/api/platform/teacher/exams/{id}/questions/{qid}/correct", post(platform_exam_key::teacher_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, new_id};
    use crate::platform_exam_admin::{act, create_exam, delete_exam, duplicate, get_exam, list_exams, update_exam};
    use crate::platform_exams::{list_available, start_attempt_with, submit_attempt};
    use rusqlite::params;
    use serde_json::{json, Value};
    use std::collections::{HashMap, HashSet};

    const NOW: i64 = 1_000_000_000_000;

    struct W {
        conn: Connection,
        admin: User,
        t1: User,
        t2: User,
        student: User,
    }

    /// Institution `i1` with subjects `s1` and `s2`; `t1` approved for both, `t2` for `s1` only; one enrolled student.
    fn world() -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t1 = insert_test_user(&conn, "t1@x.com", "teacher", "active");
        let t2 = insert_test_user(&conn, "t2@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s2','i1','شبكات',0)", []).unwrap();
        for (t, s) in [(&t1, "s1"), (&t1, "s2"), (&t2, "s1")] {
            conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1, ?2, 'approved', 0, 0)", params![t.id, s]).unwrap();
        }
        conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![student.id]).unwrap();
        W { conn, admin, t1, t2, student }
    }

    fn crypto() -> Crypto {
        Crypto::for_tests()
    }

    fn switch(w: &W, on: bool) {
        let r: crate::platform_ai::SettingsReq = serde_json::from_value(json!({ "teachers_can_create_exams": on })).unwrap();
        crate::platform_ai::apply_settings(&w.conn, &crypto(), &w.admin.id, &r).unwrap();
    }

    fn qs(n: usize) -> Vec<Value> {
        (1..=n).map(|i| json!({"id": format!("q{i}"), "type": "single_choice", "stem": format!("سؤال {i}"), "options": ["أ", "ب", "ج"], "answer": "b", "analysis": "", "score": 2})).collect()
    }

    fn req(v: Value) -> ExamReq {
        serde_json::from_value(v).unwrap()
    }

    fn basic(extra: Value) -> ExamReq {
        let mut v = json!({"subject_id": "s1", "title": "امتحان", "questions": qs(3)});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        req(v)
    }

    fn code(e: &crate::relay::Err) -> String {
        serde_json::from_str::<Value>(&e.1).unwrap()["error"].as_str().unwrap_or("").to_string()
    }

    /// `(status, error code)` of a failed call.
    fn fail<T: std::fmt::Debug>(r: Res<T>) -> (u16, String) {
        let e = r.unwrap_err();
        (e.0.as_u16(), code(&e))
    }

    fn mk(w: &W, who: &User, extra: Value) -> String {
        create_exam(&w.conn, who, &basic(extra), NOW).unwrap().info_id()
    }

    fn row(w: &W, who: &User, id: &str) -> engine::ExamRow {
        get_exam(&w.conn, who, id, NOW).unwrap().row
    }

    /// A submitted attempt by a fresh enrolled student (enough for the freeze and deletion rules).
    fn add_attempt(w: &W, exam: &str) -> String {
        let student = insert_test_user(&w.conn, &format!("{}@x.com", new_id()), "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![student.id]).ok();
        let id = new_id();
        w.conn.execute("INSERT INTO attempts(id, assessment_id, student_id, started_at, status, submitted_at) VALUES (?1, ?2, ?3, ?4, 'submitted', ?4)", params![id, exam, student.id, NOW]).unwrap();
        id
    }

    fn by(w: &W, id: &str) -> (Option<String>, Option<String>) {
        w.conn.query_row("SELECT closed_by, archived_by FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    fn status_of(w: &W, id: &str) -> String {
        w.conn.query_row("SELECT status FROM assessments WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    }

    fn count(w: &W, sql: &str) -> i64 {
        w.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn run(w: &W, who: &User, id: &str, action: &str) -> Res<(StatusCode, ExamDetail)> {
        lifecycle_checked(&w.conn, &crypto(), who, id, action, NOW)
    }

    // ───── creating ─────

    #[test]
    fn a_teacher_creates_a_draft_they_own_and_a_published_one_announces_once() {
        let w = world();
        let d = create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({"sources": {"q1": "bank-1", "ghost": "x"}})), NOW).unwrap();
        let i = &d.row.info;
        assert_eq!((i.teacher_id.clone(), i.created_by.clone(), i.status.as_str(), i.question_count, i.total_points), (Some(w.t1.id.clone()), Some(w.t1.id.clone()), "draft", 3, 6.0));
        assert_eq!((d.row.owned, d.row.can_edit, d.row.locked, d.row.phase.as_str(), d.row.visible, d.row.hidden_reason), (true, true, false, "draft", false, None));
        assert_eq!((d.questions[0].answer.as_str(), d.sources.len()), ("B", 1), "answers are normalised and stray sources dropped, like the admin builder");
        assert_eq!(count(&w, "SELECT count(*) FROM notifications"), 0, "a draft announces nothing");
        // published on create: the enrolled student is told, once
        let p = create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({"status": "published", "title": "منشور"})), NOW).unwrap();
        assert_eq!((p.row.info.status.as_str(), p.row.visible), ("published", true));
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'new_assessment'"), 1);
        run(&w, &w.t1, &p.row.info.id, "unpublish").unwrap();
        run(&w, &w.t1, &p.row.info.id, "publish").unwrap();
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'new_assessment'"), 1, "publishing again does not announce again");
        let log = count(&w, "SELECT count(*) FROM audit_log WHERE action IN ('exam_create','exam_publish') AND actor_id = (SELECT id FROM users WHERE email = 't1@x.com')");
        assert!(log >= 3, "creation and publication are audited with the teacher as the actor");
    }

    #[test]
    fn creating_checks_the_switch_then_the_assignment_then_the_content() {
        let w = world();
        // 1. the switch comes first, even for a request that would also fail every later check
        switch(&w, false);
        let junk = basic(json!({"subject_id": "s9", "title": "  ", "questions": []}));
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &junk, NOW)), (403, "exams_disabled".into()));
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({})), NOW)), (403, "exams_disabled".into()));
        assert_eq!(count(&w, "SELECT count(*) FROM assessments"), 0);
        switch(&w, true);
        // 2. then the assignment (before the content is even looked at)
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &junk, NOW)), (403, "not_assigned".into()), "unknown subject = not assigned");
        let outsider = insert_test_user(&w.conn, "t3@x.com", "teacher", "active");
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &outsider, &basic(json!({"title": " "})), NOW)), (403, "not_assigned".into()));
        for (label, sql) in [
            ("pending", "UPDATE teacher_subjects SET status = 'pending' WHERE teacher_id = (SELECT id FROM users WHERE email='t1@x.com') AND subject_id = 's2'"),
            ("rejected", "UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = (SELECT id FROM users WHERE email='t1@x.com') AND subject_id = 's2'"),
        ] {
            w.conn.execute(sql, []).unwrap();
            assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({"subject_id": "s2"})), NOW)), (403, "not_assigned".into()), "{label}");
        }
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({})), NOW)), (403, "not_assigned".into()), "inactive subject");
        w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        for status in ["draft", "published"] {
            assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({"status": status})), NOW)), (403, "not_assigned".into()), "hidden institution, {status}");
        }
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        // 3. then the content
        assert_eq!(fail(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({"title": " "})), NOW)), (400, "invalid_title".into()));
        assert!(create_checked(&w.conn, &crypto(), &w.t1, &basic(json!({})), NOW).is_ok());
    }

    #[test]
    fn a_teacher_gets_exactly_the_admin_builders_validation() {
        let w = world();
        let mut bad_answer = qs(3);
        bad_answer[1]["answer"] = json!("Z");
        let mut dup = qs(2);
        dup[1]["id"] = json!("q1");
        let mut tf_bad = qs(1);
        tf_bad[0] = json!({"id": "t", "type": "true_false", "stem": "ص", "answer": "ربما"});
        let mut bad_fill = qs(1);
        bad_fill[0] = json!({"id": "f", "type": "fill_blank", "stem": "ف", "answer": "  "});
        let cases: Vec<(&str, Value)> = vec![
            ("duration 0", json!({"duration_min": 0})), ("duration 481", json!({"duration_min": 481})), ("attempts 0", json!({"max_attempts": 0})), ("attempts 11", json!({"max_attempts": 11})),
            ("pass 0", json!({"pass_mark": 0})), ("pass 101", json!({"pass_mark": 101})), ("release", json!({"release_mode": "never"})),
            ("after_close without close", json!({"release_mode": "after_close"})), ("window reversed", json!({"opens_at": NOW + 10, "closes_at": NOW})),
            ("status", json!({"status": "closed"})), ("empty title", json!({"title": "  "})), ("answer outside the options", json!({"questions": bad_answer})),
            ("duplicate ids", json!({"questions": dup})), ("201 questions", json!({"questions": qs(201)})), ("true/false answer nobody can grade", json!({"questions": tf_bad})),
            ("blank fill-blank answer", json!({"questions": bad_fill})), ("publish empty", json!({"status": "published", "questions": []})),
            ("publish into the past", json!({"status": "published", "closes_at": NOW - 1})),
        ];
        for (label, extra) in cases {
            let t = fail(create_exam(&w.conn, &w.t1, &basic(extra.clone()), NOW));
            let a = fail(create_exam(&w.conn, &w.admin, &basic(extra), NOW));
            assert_eq!(t, a, "{label}: teacher and admin must be refused the same way");
            assert_eq!(t.0, 400, "{label}");
        }
        // the offending question is named, as for the admin
        let mut flawed = qs(3);
        flawed[1]["answer"] = json!("Z");
        let e = create_exam(&w.conn, &w.t1, &basic(json!({ "questions": flawed })), NOW).unwrap_err();
        let v: Value = serde_json::from_str(&e.1).unwrap();
        assert_eq!((v["error"].as_str(), v["question_id"].as_str(), v["index"].as_i64()), (Some("invalid_answer"), Some("q2"), Some(1)));
        assert_eq!(count(&w, "SELECT count(*) FROM assessments"), 0, "nothing was stored by any of the refused requests");
    }

    // ───── ownership ─────

    #[test]
    fn nobody_but_the_owner_reaches_a_teachers_exam_and_a_missing_exam_is_404() {
        let w = world();
        let mine = mk(&w, &w.t1, json!({"closes_at": NOW + 10_000}));
        for action in ["publish", "unpublish", "close", "reopen", "archive", "restore", "duplicate"] {
            assert_eq!(fail(run(&w, &w.t2, &mine, action)), (403, "forbidden".into()), "another teacher: {action}");
        }
        assert_eq!(fail(get_exam(&w.conn, &w.t2, &mine, NOW)), (403, "forbidden".into()));
        assert_eq!(fail(update_exam(&w.conn, &w.t2, &mine, &req(json!({"title": "اختراق"})), NOW)), (403, "forbidden".into()));
        assert_eq!(fail(delete_exam(&w.conn, &w.t2, &mine, None)), (403, "forbidden".into()));
        assert_eq!(fail(delete_exam(&w.conn, &w.t2, &mine, Some("امتحان"))), (403, "forbidden".into()), "no title makes a stranger the owner");
        assert_eq!((status_of(&w, &mine), count(&w, "SELECT count(*) FROM assessments")), ("draft".to_string(), 1), "nothing happened");
        // the same walls hold on an exam of the admin's
        let theirs = mk(&w, &w.admin, json!({}));
        for action in ["publish", "unpublish", "close", "reopen", "archive", "restore", "duplicate"] {
            assert_eq!(fail(run(&w, &w.t1, &theirs, action)), (403, "forbidden".into()), "admin exam: {action}");
        }
        assert_eq!(fail(get_exam(&w.conn, &w.t1, &theirs, NOW)), (403, "forbidden".into()), "a teacher reads the admin's exam through results / grading, not the builder");
        assert_eq!(fail(update_exam(&w.conn, &w.t1, &theirs, &req(json!({"title": "x"})), NOW)), (403, "forbidden".into()));
        assert_eq!(fail(delete_exam(&w.conn, &w.t1, &theirs, None)), (403, "forbidden".into()));
        // missing ids
        for r in [fail(get_exam(&w.conn, &w.t1, "ghost", NOW)), fail(update_exam(&w.conn, &w.t1, "ghost", &req(json!({})), NOW)), fail(delete_exam(&w.conn, &w.t1, "ghost", None)), fail(run(&w, &w.t1, "ghost", "close")), fail(run(&w, &w.t1, "ghost", "duplicate"))] {
            assert_eq!(r, (404, "not_found".into()));
        }
        // students and the like never reach the engine's writes at all
        assert_eq!(fail(create_exam(&w.conn, &w.student, &basic(json!({})), NOW)), (403, "forbidden".into()));
        assert_eq!(fail(list_exams(&w.conn, &w.student, &Default::default(), NOW)), (403, "forbidden".into()));
    }

    #[test]
    fn an_admin_moderates_a_teachers_exam_but_never_edits_it() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"closes_at": NOW + 100_000}));
        let d = get_exam(&w.conn, &w.admin, &id, NOW).unwrap();
        assert_eq!((d.row.owned, d.row.can_edit, d.questions.len()), (false, false, 3), "the admin sees it, with its answers, but cannot edit it");
        assert_eq!(fail(update_exam(&w.conn, &w.admin, &id, &req(json!({"title": "x"})), NOW)), (403, "forbidden".into()));
        assert_eq!(fail(act(&w.conn, &w.admin, &id, "publish", NOW)), (403, "forbidden".into()), "publishing is the owner's decision");
        run(&w, &w.t1, &id, "publish").unwrap();
        // moderation: unpublish (no attempts), close, archive, restore, reopen — each stamps / clears the actor
        assert_eq!(act(&w.conn, &w.admin, &id, "unpublish", NOW).unwrap().row.info.status, "draft");
        run(&w, &w.t1, &id, "publish").unwrap();
        let closed = act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
        assert_eq!((closed.row.info.status.as_str(), closed.row.locked, closed.row.can_edit), ("closed", true, false));
        assert_eq!(by(&w, &id), (Some(w.admin.id.clone()), None));
        let reopened = act(&w.conn, &w.admin, &id, "reopen", NOW + 2).unwrap();
        assert_eq!((reopened.row.info.status.as_str(), reopened.row.locked, reopened.row.can_edit), ("published", false, false), "(the admin still cannot edit it)");
        assert!(row(&w, &w.t1, &id).can_edit, "…but its owner can again");
        assert_eq!(by(&w, &id), (None, None), "reopening clears who closed it");
        act(&w.conn, &w.admin, &id, "close", NOW + 3).unwrap();
        let archived = act(&w.conn, &w.admin, &id, "archive", NOW + 4).unwrap();
        assert_eq!((archived.row.info.status.as_str(), archived.row.locked), ("archived", true));
        assert_eq!(by(&w, &id), (Some(w.admin.id.clone()), Some(w.admin.id.clone())));
        let restored = act(&w.conn, &w.admin, &id, "restore", NOW + 5).unwrap();
        assert_eq!(restored.row.info.status, "closed", "it ran, so it comes back closed");
        assert_eq!(by(&w, &id), (Some(w.admin.id.clone()), None), "restoring clears the archiver, keeps the closer");
        assert!(restored.row.locked, "…and an admin-closed exam stays locked for its owner");
        // copy: an admin-owned draft; delete: needs no title when nobody sat it
        let copy = duplicate(&w.conn, &w.admin, &id, NOW).unwrap();
        assert_eq!((copy.row.info.teacher_id.clone(), copy.row.owned, copy.row.info.status.as_str()), (None, true, "draft"));
        add_attempt(&w, &id);
        assert_eq!(fail(delete_exam(&w.conn, &w.admin, &id, None)), (409, "confirm_required".into()));
        assert_eq!(fail(delete_exam(&w.conn, &w.admin, &id, Some("غير ذلك"))), (409, "confirm_required".into()));
        delete_exam(&w.conn, &w.admin, &id, Some("امتحان")).unwrap();
        let log: String = w.conn.query_row("SELECT group_concat(detail) FROM audit_log WHERE action = 'exam_delete'", [], |r| r.get(0)).unwrap();
        assert!(log.contains("attempts=1"), "{log}");
    }

    // ───── lifecycle ─────

    #[test]
    fn the_teachers_lifecycle_walks_the_whole_state_machine_and_stamps_the_actor() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"closes_at": NOW + 10_000}));
        let st = |a: &str| run(&w, &w.t1, &id, a);
        for (label, action) in [("close a draft", "close"), ("reopen a draft", "reopen"), ("restore a draft", "restore"), ("unpublish a draft", "unpublish")] {
            assert_eq!(fail(st(action)), (409, "invalid_transition".into()), "{label}");
        }
        assert_eq!(fail(st("explode")), (400, "invalid_action".into()));
        let (code, d) = st("publish").unwrap();
        assert_eq!((code, d.row.info.status.as_str(), d.row.visible), (StatusCode::OK, "published", true));
        assert_eq!(fail(st("publish")), (409, "invalid_transition".into()));
        assert_eq!(fail(st("archive")), (409, "close_first".into()), "a live exam is closed before it is archived");
        assert_eq!(fail(st("reopen")), (409, "invalid_transition".into()));
        // closing stamps the teacher, so the exam is NOT locked for them
        let c = st("close").unwrap().1;
        assert_eq!((c.row.info.status.as_str(), c.row.locked, c.row.can_edit, c.row.info.closed_at), ("closed", false, true, Some(NOW)));
        assert_eq!(by(&w, &id), (Some(w.t1.id.clone()), None));
        assert_eq!(fail(st("close")), (409, "invalid_transition".into()));
        assert_eq!(st("reopen").unwrap().1.row.info.status, "published");
        assert_eq!(by(&w, &id), (None, None));
        // a closing time that has passed must be extended before reopening
        st("close").unwrap();
        assert_eq!(fail(lifecycle_checked(&w.conn, &crypto(), &w.t1, &id, "reopen", NOW + 20_000)), (400, "invalid_time".into()));
        update_exam(&w.conn, &w.t1, &id, &req(json!({"closes_at": NOW + 99_000})), NOW + 20_000).unwrap();
        assert_eq!(lifecycle_checked(&w.conn, &crypto(), &w.t1, &id, "reopen", NOW + 20_000).unwrap().1.row.info.status, "published");
        // unpublish is refused once a student sat it, the way out is closing
        add_attempt(&w, &id);
        assert_eq!(fail(st("unpublish")), (409, "has_attempts".into()));
        st("close").unwrap();
        let a = st("archive").unwrap().1;
        assert_eq!((a.row.info.status.as_str(), a.row.locked, a.row.can_edit, a.row.phase.as_str()), ("archived", false, false, "archived"));
        assert_eq!(by(&w, &id), (Some(w.t1.id.clone()), Some(w.t1.id.clone())));
        assert_eq!(fail(st("archive")), (409, "invalid_transition".into()));
        assert_eq!(fail(update_exam(&w.conn, &w.t1, &id, &req(json!({"title": "x"})), NOW)), (409, "archived".into()), "archived exams are read-only");
        assert_eq!(st("restore").unwrap().1.row.info.status, "closed", "an exam that ran comes back closed");
        assert_eq!(by(&w, &id), (Some(w.t1.id.clone()), None));
        // a never-run exam comes back as a draft
        let fresh = mk(&w, &w.t1, json!({"title": "جديد"}));
        run(&w, &w.t1, &fresh, "archive").unwrap();
        assert_eq!(run(&w, &w.t1, &fresh, "restore").unwrap().1.row.info.status, "draft");
        let log: String = w.conn.query_row("SELECT group_concat(action) FROM audit_log", [], |r| r.get(0)).unwrap();
        for a in ["exam_create", "exam_publish", "exam_close", "exam_reopen", "exam_archive", "exam_restore", "exam_update"] {
            assert!(log.contains(a), "{a} is audited");
        }
    }

    #[test]
    fn the_switch_gates_creating_publishing_and_duplicating_but_never_managing_a_live_exam() {
        let w = world();
        let live = mk(&w, &w.t1, json!({"status": "published", "closes_at": NOW + 100_000}));
        let draft = mk(&w, &w.t1, json!({"title": "مسودة"}));
        add_attempt(&w, &live);
        switch(&w, false);
        assert_eq!(fail(run(&w, &w.t1, &draft, "publish")), (403, "exams_disabled".into()));
        assert_eq!(fail(run(&w, &w.t1, &live, "duplicate")), (403, "exams_disabled".into()));
        assert_eq!(status_of(&w, &draft), "draft");
        // everything else keeps working with the switch off
        update_exam(&w.conn, &w.t1, &live, &req(json!({"title": "عنوان معدل", "closes_at": NOW + 200_000})), NOW).unwrap();
        update_exam(&w.conn, &w.t1, &draft, &req(json!({"title": "مسودة معدلة"})), NOW).unwrap();
        run(&w, &w.t1, &live, "close").unwrap();
        run(&w, &w.t1, &live, "reopen").expect("reopening does not need the switch");
        run(&w, &w.t1, &live, "close").unwrap();
        run(&w, &w.t1, &live, "archive").unwrap();
        run(&w, &w.t1, &live, "restore").unwrap();
        delete_exam(&w.conn, &w.t1, &draft, None).unwrap();
        // someone else's exam is still a plain 403, not a hint about the switch
        assert_eq!(fail(run(&w, &w.t2, &live, "publish")), (403, "forbidden".into()));
        switch(&w, true);
        let copy = run(&w, &w.t1, &live, "duplicate").unwrap();
        assert_eq!(copy.0, StatusCode::CREATED);
    }

    #[test]
    fn publishing_and_reopening_need_the_owners_live_assignment() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"subject_id": "s2", "closes_at": NOW + 100_000}));
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE subject_id = 's2'", []).unwrap();
        assert_eq!(fail(run(&w, &w.t1, &id, "publish")), (403, "not_assigned".into()));
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE subject_id = 's2'", []).unwrap();
        run(&w, &w.t1, &id, "publish").unwrap();
        run(&w, &w.t1, &id, "close").unwrap();
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE subject_id = 's2'", []).unwrap();
        assert_eq!(fail(run(&w, &w.t1, &id, "reopen")), (403, "not_assigned".into()), "it would go live into a subject the teacher no longer has");
        assert_eq!(fail(act(&w.conn, &w.admin, &id, "reopen", NOW)), (403, "not_assigned".into()), "an admin reopening it is subject to the same rule");
        // closing, archiving and restoring never put anything in front of students
        run(&w, &w.t1, &id, "archive").unwrap();
        run(&w, &w.t1, &id, "restore").unwrap();
        // a hidden institution is the same
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE subject_id = 's2'", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(fail(run(&w, &w.t1, &id, "reopen")), (403, "not_assigned".into()));
        w.conn.execute("UPDATE institutions SET is_active = 1", []).unwrap();
        assert_eq!(run(&w, &w.t1, &id, "reopen").unwrap().1.row.info.status, "published");
    }

    #[test]
    fn duplicating_makes_the_teachers_own_fresh_draft_even_from_a_locked_exam() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"closes_at": NOW + 9000, "opens_at": NOW, "release_mode": "after_close", "pass_mark": 50, "shuffle_options": true, "sources": {"q1": "b1"}, "status": "published"}));
        add_attempt(&w, &id);
        act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
        assert!(row(&w, &w.t1, &id).locked);
        let (code, c) = run(&w, &w.t1, &id, "duplicate").unwrap();
        let i = &c.row.info;
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!((i.title.as_str(), i.status.as_str(), i.opens_at, i.closes_at, i.attempt_count, i.pass_mark, i.shuffle_options), ("امتحان (نسخة)", "draft", None, None, 0, Some(50.0), true));
        assert_eq!((i.release_mode.as_str(), c.questions.len(), c.sources.len()), ("immediate", 3, 1));
        assert_eq!((i.teacher_id.clone(), i.created_by.clone(), c.row.owned, c.row.locked, c.row.can_edit), (Some(w.t1.id.clone()), Some(w.t1.id.clone()), true, false, true), "the copy belongs to the teacher and is theirs to edit");
        // needs an approved assignment for the exam's subject
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's1'", params![w.t1.id]).unwrap();
        assert_eq!(fail(run(&w, &w.t1, &id, "duplicate")), (403, "not_assigned".into()));
        // long titles are trimmed to fit
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1 AND subject_id = 's1'", params![w.t1.id]).unwrap();
        let long = mk(&w, &w.t1, json!({"title": "ع".repeat(200)}));
        assert_eq!(run(&w, &w.t1, &long, "duplicate").unwrap().1.row.info.title.chars().count(), 200);
        let log = count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_duplicate'");
        assert_eq!(log, 2);
    }

    // ───── locked ─────

    /// A teacher's closed exam with `closed_by` set the way each origin would.
    fn closed_exam(w: &W, origin: &str) -> String {
        let id = mk(w, &w.t1, json!({"status": "published", "closes_at": NOW + 100_000, "title": origin}));
        match origin {
            "owner" => {
                run(w, &w.t1, &id, "close").unwrap();
            }
            "admin" => {
                act(&w.conn, &w.admin, &id, "close", NOW).unwrap();
            }
            "legacy" | "system" => {
                // a row from before the actor was recorded / the report auto-hide: closed with no actor
                w.conn.execute("UPDATE assessments SET status = 'closed', closed_at = ?2, closed_by = NULL WHERE id = ?1", params![id, NOW]).unwrap();
            }
            "ghost" => {
                // closed by an account that no longer exists: its id is not the owner's
                w.conn.execute("UPDATE assessments SET status = 'closed', closed_at = ?2, closed_by = 'deleted-admin' WHERE id = ?1", params![id, NOW]).unwrap();
            }
            _ => unreachable!(),
        }
        id
    }

    #[test]
    fn who_closed_it_decides_whether_the_owner_is_locked_out() {
        let w = world();
        for (origin, locked) in [("owner", false), ("admin", true), ("legacy", true), ("system", true), ("ghost", true)] {
            let id = closed_exam(&w, origin);
            let r = row(&w, &w.t1, &id);
            assert_eq!((r.locked, r.can_edit), (locked, !locked), "{origin}");
            // the verdict is about the exam, not the viewer: the admin and the list see the same flag
            assert_eq!(row(&w, &w.admin, &id).locked, locked, "{origin} seen by an admin");
            let listed = list_exams(&w.conn, &w.t1, &ListQuery { status: Some("closed".into()), q: Some(origin.into()), ..Default::default() }, NOW).unwrap().items;
            assert_eq!((listed.len(), listed[0].locked), (1, locked), "{origin} in the list");
            let edit = update_exam(&w.conn, &w.t1, &id, &req(json!({"title": format!("{origin} معدل")})), NOW);
            if locked {
                assert_eq!(fail(edit), (409, "locked".into()), "{origin}: edit");
                for action in ["publish", "unpublish", "close", "reopen", "archive", "restore", "explode"] {
                    let expected = if action == "explode" { (400, "invalid_action".to_string()) } else { (409, "locked".to_string()) };
                    assert_eq!(fail(run(&w, &w.t1, &id, action)), expected, "{origin}: {action}");
                }
                assert_eq!(status_of(&w, &id), "closed", "{origin}: nothing moved");
                // what the owner CAN still do: copy it, and delete it when nobody sat it
                assert_eq!(run(&w, &w.t1, &id, "duplicate").unwrap().0, StatusCode::CREATED, "{origin}: duplicate");
            } else {
                edit.unwrap();
                assert_eq!(run(&w, &w.t1, &id, "reopen").unwrap().1.row.info.status, "published", "{origin}: reopen");
            }
        }
    }

    #[test]
    fn only_an_admin_lifts_a_lock_and_archiving_by_an_admin_locks_too() {
        let w = world();
        let id = closed_exam(&w, "admin");
        assert_eq!(fail(run(&w, &w.t1, &id, "reopen")), (409, "locked".into()));
        let r = act(&w.conn, &w.admin, &id, "reopen", NOW + 1).unwrap();
        assert_eq!((r.row.info.status.as_str(), r.row.locked, row(&w, &w.t1, &id).can_edit), ("published", false, true));
        update_exam(&w.conn, &w.t1, &id, &req(json!({"title": "عاد للمعلم"})), NOW).unwrap();
        // the teacher closes it, an admin archives it: the ARCHIVE is what locks, and restoring unlocks again
        run(&w, &w.t1, &id, "close").unwrap();
        let a = act(&w.conn, &w.admin, &id, "archive", NOW + 2).unwrap();
        assert_eq!((a.row.locked, by(&w, &id)), (true, (Some(w.t1.id.clone()), Some(w.admin.id.clone()))));
        for action in ["restore", "publish", "close"] {
            assert_eq!(fail(run(&w, &w.t1, &id, action)), (409, "locked".into()), "archived by an admin: {action}");
        }
        assert_eq!(fail(update_exam(&w.conn, &w.t1, &id, &req(json!({"title": "x"})), NOW)), (409, "archived".into()), "an archived exam is read-only whoever archived it");
        let back = act(&w.conn, &w.admin, &id, "restore", NOW + 3).unwrap();
        assert_eq!((back.row.info.status.as_str(), back.row.locked), ("closed", false), "the teacher closed it themself, so it is theirs again");
        // an exam the TEACHER archived (and that nobody else touched) can be restored by them
        let mine = mk(&w, &w.t1, json!({"title": "ملكي"}));
        run(&w, &w.t1, &mine, "archive").unwrap();
        assert!(!row(&w, &w.t1, &mine).locked);
        assert_eq!(run(&w, &w.t1, &mine, "restore").unwrap().1.row.info.status, "draft");
        // a draft an admin archived is locked until an admin restores it
        let drafted = mk(&w, &w.t1, json!({"title": "مسودة المشرف"}));
        act(&w.conn, &w.admin, &drafted, "archive", NOW).unwrap();
        assert_eq!(fail(run(&w, &w.t1, &drafted, "restore")), (409, "locked".into()));
        assert_eq!(act(&w.conn, &w.admin, &drafted, "restore", NOW).unwrap().row.info.status, "draft");
        // admin exams are never locked, whoever closed them
        let admin_exam = mk(&w, &w.admin, json!({"status": "published"}));
        act(&w.conn, &w.admin, &admin_exam, "close", NOW).unwrap();
        w.conn.execute("UPDATE assessments SET closed_by = NULL WHERE id = ?1", params![admin_exam]).unwrap();
        assert!(!row(&w, &w.admin, &admin_exam).locked);
    }

    #[test]
    fn an_admin_can_always_hand_a_locked_exam_back_even_when_its_closing_time_has_passed() {
        let w = world();
        // the situation reopening cannot fix: an admin closed a sat exam and its closing time has passed since
        let id = mk(&w, &w.t1, json!({"status": "published", "closes_at": NOW + 100_000}));
        add_attempt(&w, &id);
        act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
        let later = NOW + 200_000;
        assert_eq!(fail(act(&w.conn, &w.admin, &id, "reopen", later)), (400, "invalid_time".into()), "reopening needs a closing time ahead");
        assert_eq!(fail(update_exam(&w.conn, &w.admin, &id, &req(json!({"closes_at": later + 99_000})), later)), (403, "forbidden".into()), "an admin never edits it");
        assert_eq!(fail(update_exam(&w.conn, &w.t1, &id, &req(json!({"closes_at": later + 99_000})), later)), (409, "locked".into()), "…and the owner is locked out");
        assert_eq!(fail(run(&w, &w.t1, &id, "unlock")), (409, "locked".into()), "a teacher cannot unlock themself");
        // unlock: no state change, the owner is theirs again
        let u = act(&w.conn, &w.admin, &id, "unlock", later).unwrap();
        assert_eq!((u.row.info.status.as_str(), u.row.locked, u.row.can_edit, u.row.owned), ("closed", false, false, false), "still closed; the admin still does not own it");
        assert_eq!(by(&w, &id), (Some(w.t1.id.clone()), None));
        let mine = row(&w, &w.t1, &id);
        assert_eq!((mine.locked, mine.can_edit), (false, true));
        // the way back to a live exam is the owner's: extend the closing time, then reopen
        update_exam(&w.conn, &w.t1, &id, &req(json!({"closes_at": later + 99_000})), later).unwrap();
        assert_eq!(lifecycle_checked(&w.conn, &crypto(), &w.t1, &id, "reopen", later).unwrap().1.row.info.status, "published");
        // unlocking something that is not locked is refused, and so is anyone but an admin
        assert_eq!(fail(act(&w.conn, &w.admin, &id, "unlock", later)), (409, "invalid_transition".into()), "a live exam");
        run(&w, &w.t1, &id, "close").unwrap();
        assert_eq!(fail(act(&w.conn, &w.admin, &id, "unlock", later)), (409, "invalid_transition".into()), "closed by its owner: nothing to lift");
        assert_eq!(fail(run(&w, &w.t1, &id, "unlock")), (403, "forbidden".into()), "an owner whose exam is not locked is still not an admin");
        assert_eq!(fail(run(&w, &w.t2, &id, "unlock")), (403, "forbidden".into()), "another teacher");
        assert_eq!(fail(act(&w.conn, &w.student, &id, "unlock", later)), (403, "forbidden".into()), "a student");
        assert_eq!(fail(act(&w.conn, &w.admin, "ghost", "unlock", later)), (404, "not_found".into()));
        let admin_exam = mk(&w, &w.admin, json!({"status": "published"}));
        act(&w.conn, &w.admin, &admin_exam, "close", NOW).unwrap();
        assert_eq!(fail(act(&w.conn, &w.admin, &admin_exam, "unlock", later)), (409, "invalid_transition".into()), "an admin exam is never locked");
        assert_eq!(count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_unlock'"), 1, "only the real unlock is audited");
    }

    #[test]
    fn unlocking_covers_rows_from_before_the_actor_was_recorded_the_system_and_archived_exams() {
        let w = world();
        // legacy / report auto-hide: closed with no recorded actor, and nobody can reopen it any more
        for origin in ["legacy", "system", "ghost"] {
            let id = closed_exam(&w, origin);
            assert!(row(&w, &w.t1, &id).locked, "{origin}");
            act(&w.conn, &w.admin, &id, "unlock", NOW + 1_000_000).unwrap();
            assert_eq!((by(&w, &id), row(&w, &w.t1, &id).locked), ((Some(w.t1.id.clone()), None), false), "{origin}");
            assert_eq!(run(&w, &w.t1, &id, "archive").unwrap().1.row.info.status, "archived", "{origin}: the owner manages it again");
        }
        // archived by an admin after an admin closed it: BOTH records name the owner afterwards, so restoring does not lock again
        let id = mk(&w, &w.t1, json!({"status": "published", "closes_at": NOW + 100_000}));
        add_attempt(&w, &id);
        act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
        act(&w.conn, &w.admin, &id, "archive", NOW + 2).unwrap();
        assert_eq!(by(&w, &id), (Some(w.admin.id.clone()), Some(w.admin.id.clone())));
        let u = act(&w.conn, &w.admin, &id, "unlock", NOW + 3).unwrap();
        assert_eq!((u.row.info.status.as_str(), u.row.locked, u.row.phase.as_str()), ("archived", false, "archived"));
        assert_eq!(by(&w, &id), (Some(w.t1.id.clone()), Some(w.t1.id.clone())));
        let back = run(&w, &w.t1, &id, "restore").unwrap().1;
        assert_eq!((back.row.info.status.as_str(), back.row.locked, back.row.can_edit), ("closed", false, true));
        // a draft an admin archived: unlocking lets its owner restore it as a draft
        let draft = mk(&w, &w.t1, json!({"title": "مسودة المشرف"}));
        act(&w.conn, &w.admin, &draft, "archive", NOW).unwrap();
        act(&w.conn, &w.admin, &draft, "unlock", NOW).unwrap();
        assert_eq!(by(&w, &draft), (None, Some(w.t1.id.clone())), "it was never closed, so there is no closer to name");
        assert_eq!(run(&w, &w.t1, &draft, "restore").unwrap().1.row.info.status, "draft");
    }

    #[test]
    fn an_admin_changing_a_teachers_exam_tells_its_owner_and_nobody_else() {
        let w = world();
        let kinds = |w: &W| -> Vec<(String, String)> {
            w.conn
                .prepare("SELECT n.kind, u.email FROM notifications n JOIN users u ON u.id = n.user_id WHERE n.kind NOT IN ('new_assessment') ORDER BY n.created_at, n.rowid")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let id = mk(&w, &w.t1, json!({"status": "published", "closes_at": NOW + 100_000}));
        // the owner's own actions tell nobody
        run(&w, &w.t1, &id, "close").unwrap();
        run(&w, &w.t1, &id, "reopen").unwrap();
        assert!(kinds(&w).is_empty());
        // the admin's: unpublish keeps its old wording; close / archive and the three ways back have their own
        let mut expected: Vec<(String, String)> = vec![];
        let mut step = |w: &W, who: &User, action: &str, kind: Option<&str>| {
            if who.role == "admin" { act(&w.conn, who, &id, action, NOW + 1).unwrap(); } else { run(w, who, &id, action).unwrap(); }
            if let Some(k) = kind {
                expected.push((k.to_string(), "t1@x.com".to_string()));
            }
            assert_eq!(kinds(w), expected, "after {} by {}", action, who.role);
        };
        step(&w, &w.admin, "unpublish", Some("content_unpublished"));
        step(&w, &w.t1, "publish", None);
        step(&w, &w.admin, "close", Some("exam_admin_closed"));
        step(&w, &w.admin, "reopen", Some("exam_admin_released"));
        step(&w, &w.admin, "close", Some("exam_admin_closed"));
        step(&w, &w.admin, "archive", Some("exam_admin_archived"));
        step(&w, &w.admin, "restore", Some("exam_admin_released"));
        assert!(row(&w, &w.t1, &id).locked, "restoring keeps the admin as the closer");
        step(&w, &w.admin, "unlock", Some("exam_admin_released"));
        let n: String = w.conn.query_row("SELECT data FROM notifications WHERE kind = 'exam_admin_closed' LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&n).unwrap(), json!({"title": "امتحان"}));
        let link: String = w.conn.query_row("SELECT link FROM notifications WHERE kind = 'exam_admin_closed' LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(link, format!("/platform/assessments/{id}"));
        // an admin's own exam has no owner to tell; neither does a failed action
        let theirs = mk(&w, &w.admin, json!({"status": "published"}));
        let before = kinds(&w).len();
        act(&w.conn, &w.admin, &theirs, "close", NOW).unwrap();
        assert_eq!(fail(act(&w.conn, &w.admin, &theirs, "close", NOW)), (409, "invalid_transition".into()));
        assert_eq!(kinds(&w).len(), before);
    }

    #[test]
    fn a_locked_exam_can_be_deleted_by_its_owner_only_while_nobody_sat_it() {
        let w = world();
        let id = closed_exam(&w, "admin");
        add_attempt(&w, &id);
        assert_eq!(fail(delete_exam(&w.conn, &w.t1, &id, None)), (409, "has_attempts".into()));
        let spare = closed_exam(&w, "system");
        delete_exam(&w.conn, &w.t1, &spare, None).unwrap();
        assert_eq!(count(&w, &format!("SELECT count(*) FROM assessments WHERE id = '{spare}'")), 0);
    }

    // ───── deleting & freezing ─────

    #[test]
    fn a_teacher_can_never_delete_an_exam_students_have_sat_and_has_no_title_override() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"status": "published"}));
        let attempt = add_attempt(&w, &id);
        for confirm in [None, Some("امتحان"), Some("  امتحان  ")] {
            assert_eq!(fail(delete_exam(&w.conn, &w.t1, &id, confirm)), (409, "has_attempts".into()), "teacher, confirm {confirm:?}");
        }
        assert_eq!(count(&w, &format!("SELECT count(*) FROM attempts WHERE id = '{attempt}'")), 1, "the attempt survived");
        assert_eq!(count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_delete'"), 0, "a refused delete is not recorded as a deletion");
        // closing and archiving are the way out; the admin keeps the typed-title route on /admin/exams
        run(&w, &w.t1, &id, "close").unwrap();
        run(&w, &w.t1, &id, "archive").unwrap();
        assert_eq!(fail(delete_exam(&w.conn, &w.admin, &id, None)), (409, "confirm_required".into()));
        delete_exam(&w.conn, &w.admin, &id, Some("امتحان")).unwrap();
        // without attempts a teacher deletes their own, audited
        let spare = mk(&w, &w.t1, json!({"title": "لا شيء"}));
        delete_exam(&w.conn, &w.t1, &spare, None).unwrap();
        let log: String = w.conn.query_row("SELECT group_concat(detail) FROM audit_log WHERE action = 'exam_delete' AND actor_id = ?1", params![w.t1.id], |r| r.get(0)).unwrap();
        assert_eq!(log, "attempts=0");
    }

    /// The freeze rules, identical for the teacher on their exam and the admin on theirs.
    #[test]
    fn the_prd_e6_freeze_holds_for_both_actors() {
        let w = world();
        for who in [&w.t1, &w.admin] {
            let id = mk(&w, who, json!({"closes_at": NOW + 1000, "max_attempts": 2, "status": "published", "title": format!("{} مجمّد", who.role)}));
            let up = |v: Value| update_exam(&w.conn, who, &id, &req(v), NOW + 1);
            // before the first attempt everything may change
            let u = up(json!({"title": "جديد", "questions": qs(5), "duration_min": 30, "shuffle_questions": true, "shuffle_options": true, "pass_mark": 60})).unwrap();
            assert_eq!((u.row.info.question_count, u.row.info.total_points, u.row.info.duration_min, u.row.info.pass_mark), (5, 10.0, Some(30), Some(60.0)), "{}", who.role);
            assert_eq!(fail(up(json!({"status": "closed"}))), (400, "invalid_status".into()), "{}: PATCH never changes the status", who.role);
            add_attempt(&w, &id);
            for (label, v) in [("questions", json!({"questions": qs(2)})), ("duration", json!({"duration_min": 10})), ("clear duration", json!({"clear_duration": true})), ("shuffle", json!({"shuffle_options": false})),
                               ("opens", json!({"opens_at": NOW - 5})), ("sources", json!({"sources": {}})), ("subject", json!({"subject_id": "s2"}))] {
                assert_eq!(fail(up(v)), (409, "has_attempts".into()), "{}: {label} is frozen", who.role);
            }
            assert_eq!(fail(up(json!({"closes_at": NOW + 500}))), (409, "only_extend".into()), "{}: shortening", who.role);
            assert_eq!(fail(up(json!({"max_attempts": 1}))), (409, "only_extend".into()), "{}: fewer attempts", who.role);
            let ok = up(json!({"title": "عنوان معدل", "description": "وصف", "closes_at": NOW + 5000, "max_attempts": 3, "show_answers": false, "pass_mark": 50, "release_mode": "after_close"})).unwrap();
            assert_eq!((ok.row.info.title.as_str(), ok.row.info.closes_at, ok.row.info.max_attempts, ok.row.info.show_answers, ok.questions.len()), ("عنوان معدل", Some(NOW + 5000), 3, false, 5), "{}", who.role);
            assert!(up(json!({"clear_closes": true, "release_mode": "immediate"})).is_ok(), "{}: removing the closing time is an extension", who.role);
            assert_eq!(fail(delete_exam(&w.conn, who, &id, None)), if who.role == "admin" { (409, "confirm_required".into()) } else { (409, "has_attempts".into()) }, "{}", who.role);
        }
    }

    #[test]
    fn moving_an_exam_to_another_subject_needs_the_assignment_there_and_no_attempts() {
        let w = world();
        let id = mk(&w, &w.t2, json!({}));
        assert_eq!(fail(update_exam(&w.conn, &w.t2, &id, &req(json!({"subject_id": "s2"})), NOW)), (403, "not_assigned".into()), "t2 is not approved for s2");
        assert_eq!(fail(update_exam(&w.conn, &w.t2, &id, &req(json!({"subject_id": "nope"})), NOW)), (403, "not_assigned".into()));
        assert_eq!(update_exam(&w.conn, &w.t2, &id, &req(json!({"subject_id": "s1", "title": "نفس المادة"})), NOW).unwrap().row.info.subject_id, "s1", "re-sending the current subject is not a move");
        let mine = mk(&w, &w.t1, json!({}));
        assert_eq!(update_exam(&w.conn, &w.t1, &mine, &req(json!({"subject_id": "s2"})), NOW).unwrap().row.info.subject_name, "شبكات");
        add_attempt(&w, &mine);
        assert_eq!(fail(update_exam(&w.conn, &w.t1, &mine, &req(json!({"subject_id": "s1"})), NOW)), (409, "has_attempts".into()));
    }

    // ───── lists ─────

    #[test]
    fn the_teachers_list_shows_their_own_exams_by_effective_phase_with_filters_and_paging() {
        let w = world();
        let draft = mk(&w, &w.t1, json!({"title": "مسودة"}));
        let live = mk(&w, &w.t1, json!({"title": "جارٍ", "status": "published"}));
        let ending = mk(&w, &w.t1, json!({"title": "ينتهي", "status": "published", "closes_at": NOW + 100}));
        let other_subject = mk(&w, &w.t1, json!({"title": "شبكات", "subject_id": "s2"}));
        let archived = mk(&w, &w.t1, json!({"title": "قديم"}));
        run(&w, &w.t1, &archived, "archive").unwrap();
        let theirs = mk(&w, &w.t2, json!({"title": "معلم آخر"}));
        let admins = mk(&w, &w.admin, json!({"title": "للمدير", "status": "published"}));
        let ids = |f: ListQuery, now: i64| list_exams(&w.conn, &w.t1, &f, now).unwrap().items.into_iter().map(|r| r.info.id).collect::<HashSet<_>>();
        let q = |status: Option<&str>| ListQuery { status: status.map(String::from), ..Default::default() };
        assert_eq!(ids(q(None), NOW), HashSet::from([draft.clone(), live.clone(), ending.clone(), other_subject.clone()]), "mine, archived hidden by default; nobody else's");
        assert!(![&theirs, &admins].iter().any(|x| ids(q(Some("all")), NOW).contains(*x)), "another teacher's and the admin's exams are never in my own list");
        assert_eq!(ids(q(Some("all")), NOW).len(), 5);
        assert_eq!(ids(q(Some("archived")), NOW), HashSet::from([archived.clone()]));
        assert_eq!(ids(q(Some("draft")), NOW), HashSet::from([draft.clone(), other_subject.clone()]));
        assert_eq!(ids(q(Some("published")), NOW), HashSet::from([live.clone(), ending.clone()]));
        assert_eq!(ids(q(Some("published")), NOW + 500), HashSet::from([live.clone()]), "past its closing time it no longer counts as published");
        assert_eq!(ids(q(Some("closed")), NOW + 500), HashSet::from([ending.clone()]), "…it counts as closed");
        assert_eq!(ids(ListQuery { subject_id: Some("s2".into()), ..Default::default() }, NOW), HashSet::from([other_subject.clone()]));
        assert_eq!(ids(ListQuery { q: Some("مسو".into()), ..Default::default() }, NOW), HashSet::from([draft.clone()]));
        assert!(ids(ListQuery { q: Some("%".into()), ..Default::default() }, NOW).is_empty() && ids(ListQuery { q: Some("_".into()), ..Default::default() }, NOW).is_empty(), "wildcards are literal");
        assert_eq!(ids(ListQuery { scope: Some("mine".into()), ..Default::default() }, NOW).len(), 4, "scope=mine is the default");
        for bad in [q(Some("weird")), ListQuery { scope: Some("everyone".into()), ..Default::default() }] {
            assert_eq!(fail(list_exams(&w.conn, &w.t1, &bad, NOW)), (400, "invalid_filter".into()));
        }
        let p1 = list_exams(&w.conn, &w.t1, &ListQuery { limit: Some(2), status: Some("all".into()), ..Default::default() }, NOW).unwrap();
        let p2 = list_exams(&w.conn, &w.t1, &ListQuery { limit: Some(2), offset: Some(2), status: Some("all".into()), ..Default::default() }, NOW).unwrap();
        let p3 = list_exams(&w.conn, &w.t1, &ListQuery { limit: Some(2), offset: Some(4), status: Some("all".into()), ..Default::default() }, NOW).unwrap();
        assert_eq!((p1.items.len(), p2.items.len(), p3.items.len(), p1.total), (2, 2, 1, 5));
        let all: HashSet<String> = p1.items.iter().chain(&p2.items).chain(&p3.items).map(|r| r.info.id.clone()).collect();
        assert_eq!(all.len(), 5, "pages are disjoint and complete");
        // my own rows are mine, editable, with their counts
        let r = &list_exams(&w.conn, &w.t1, &q(Some("published")), NOW).unwrap().items[0];
        assert_eq!((r.owned, r.can_edit, r.locked, r.visible), (true, true, false, true));
        // the admin sees everything, in the admin list
        assert_eq!(list_exams(&w.conn, &w.admin, &q(Some("all")), NOW).unwrap().total, 7);
    }

    #[test]
    fn the_admins_exams_for_my_subjects_are_a_read_only_list_of_published_and_closed_ones() {
        let w = world();
        let published = mk(&w, &w.admin, json!({"title": "منشور", "status": "published", "subject_id": "s1"}));
        let ending = mk(&w, &w.admin, json!({"title": "ينتهي", "status": "published", "closes_at": NOW + 100}));
        let closed = mk(&w, &w.admin, json!({"title": "مغلق", "status": "published"}));
        act(&w.conn, &w.admin, &closed, "close", NOW).unwrap();
        let draft = mk(&w, &w.admin, json!({"title": "مسودة"}));
        let archived = mk(&w, &w.admin, json!({"title": "مؤرشف"}));
        act(&w.conn, &w.admin, &archived, "archive", NOW).unwrap();
        let s2 = mk(&w, &w.admin, json!({"title": "شبكات", "subject_id": "s2", "status": "published"}));
        let teachers = mk(&w, &w.t2, json!({"title": "معلم", "status": "published"}));
        let ids = |who: &User, f: ListQuery, now: i64| list_exams(&w.conn, who, &f, now).unwrap().items.into_iter().map(|r| r.info.id).collect::<HashSet<_>>();
        let scope = |status: Option<&str>| ListQuery { scope: Some("admin".into()), status: status.map(String::from), ..Default::default() };
        assert_eq!(ids(&w.t1, scope(None), NOW), HashSet::from([published.clone(), ending.clone(), closed.clone(), s2.clone()]), "published and closed only, in t1's two subjects; no drafts, no archived, no teacher exams");
        assert_eq!(ids(&w.t2, scope(None), NOW), HashSet::from([published.clone(), ending.clone(), closed.clone()]), "t2 is approved for s1 only");
        let rows = list_exams(&w.conn, &w.t1, &scope(Some("all")), NOW).unwrap();
        assert_eq!(rows.total, 4);
        assert!(rows.items.iter().all(|r| !r.owned && !r.can_edit && r.info.teacher_id.is_none()), "read-only: results and grading, never the builder");
        for st in ["draft", "archived"] {
            assert!(ids(&w.t1, scope(Some(st)), NOW).is_empty(), "the admin's {st} exams are never listed");
        }
        assert_eq!(ids(&w.t1, scope(Some("closed")), NOW + 500), HashSet::from([closed.clone(), ending.clone()]), "effective phase applies here too");
        assert_eq!(ids(&w.t1, ListQuery { q: Some("شبك".into()), ..scope(None) }, NOW), HashSet::from([s2.clone()]));
        assert_eq!(ids(&w.t1, ListQuery { subject_id: Some("s1".into()), ..scope(None) }, NOW).len(), 3);
        // approval is required: pending / rejected assignments see nothing
        w.conn.execute("UPDATE teacher_subjects SET status = 'pending' WHERE teacher_id = ?1", params![w.t2.id]).unwrap();
        assert!(ids(&w.t2, scope(None), NOW).is_empty());
        let _ = (draft, teachers);
        // the teacher gets none of it through the builder
        assert_eq!(fail(get_exam(&w.conn, &w.t1, &published, NOW)), (403, "forbidden".into()));
    }

    #[test]
    fn the_row_fields_count_submitted_attempts_and_written_answers_waiting_for_a_grade() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"status": "published", "questions": [
            {"id": "a", "type": "single_choice", "stem": "س", "options": ["x", "y"], "answer": "A", "score": 1},
            {"id": "b", "type": "short_answer", "stem": "ش", "answer": "ref", "score": 4}]}));
        let s2 = insert_test_user(&w.conn, "s2@x.com", "student", "active");
        let s3 = insert_test_user(&w.conn, "s3@x.com", "student", "active");
        for s in [&s2, &s3] {
            w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![s.id]).unwrap();
        }
        let go = |who: &User, text: &str, at: i64| {
            let st = start_attempt_with(&w.conn, who, &id, at, &mut rand::thread_rng()).unwrap();
            submit_attempt(&w.conn, who, &st.attempt_id, &[("a".to_string(), "A".to_string()), ("b".to_string(), text.to_string())].into(), at + 1).unwrap();
        };
        go(&w.student, "إجابة", NOW);
        go(&s2, "أخرى", NOW);
        go(&s3, "", NOW); // blank: nothing to grade
        start_attempt_with(&w.conn, &insert_enrolled(&w, "s4@x.com"), &id, NOW, &mut rand::thread_rng()).unwrap(); // in progress: counts in none of them
        let r = row(&w, &w.t1, &id);
        assert_eq!((r.submitted, r.pending_answers, r.info.attempt_count), (3, 2, 4));
        let v = serde_json::to_value(&r).unwrap();
        for key in ["id", "title", "status", "phase", "can_edit", "owned", "locked", "submitted", "pending_answers", "visible", "hidden_reason", "teacher_id", "attempt_count"] {
            assert!(v.get(key).is_some(), "{key} in the row JSON");
        }
        assert!(v["hidden_reason"].is_null() && v.get("closed_by").is_none() && v.get("archived_by").is_none(), "the actors stay internal; the verdict is `locked`");
        // the list agrees (same query shape, one statement per page)
        let listed = &list_exams(&w.conn, &w.t1, &ListQuery::default(), NOW).unwrap().items[0];
        assert_eq!((listed.submitted, listed.pending_answers), (3, 2));
    }

    fn insert_enrolled(w: &W, email: &str) -> User {
        let u = insert_test_user(&w.conn, email, "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![u.id]).unwrap();
        u
    }

    /// `visible` / `hidden_reason` on the builder's rows are the same answer students are served, over the whole matrix.
    #[test]
    fn row_visibility_agrees_with_what_students_see_across_the_whole_matrix() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"status": "published"}));
        let admin_exam = mk(&w, &w.admin, json!({"status": "published", "title": "للمدير"}));
        let mut cases = 0;
        for (status, role) in [("active", "teacher"), ("pending", "teacher"), ("suspended", "teacher"), ("rejected", "teacher"), ("active", "student")] {
            for assignment in [None, Some("pending"), Some("rejected"), Some("approved")] {
                for subject_on in [true, false] {
                    for institution_on in [true, false] {
                        w.conn.execute("UPDATE users SET status = ?1, role = ?2 WHERE id = ?3", params![status, role, w.t1.id]).unwrap();
                        w.conn.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1", params![w.t1.id]).unwrap();
                        if let Some(a) = assignment {
                            w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1',?2,0)", params![w.t1.id, a]).unwrap();
                        }
                        w.conn.execute("UPDATE subjects SET is_active = ?1 WHERE id = 's1'", params![subject_on]).unwrap();
                        w.conn.execute("UPDATE institutions SET is_active = ?1", params![institution_on]).unwrap();
                        let expected = if !(status == "active" && role == "teacher") {
                            Some(crate::platform_content::HiddenReason::AccountInactive)
                        } else if !institution_on {
                            Some(crate::platform_content::HiddenReason::InstitutionInactive)
                        } else if !subject_on {
                            Some(crate::platform_content::HiddenReason::SubjectInactive)
                        } else if assignment != Some("approved") {
                            Some(crate::platform_content::HiddenReason::NoAssignment)
                        } else {
                            None
                        };
                        let case = format!("{status}/{role} assignment={assignment:?} subject_on={subject_on} institution_on={institution_on}");
                        let served: HashSet<String> = list_available(&w.conn, &w.student.id).unwrap().into_iter().map(|i| i.id).collect();
                        let r = engine::list_exams(&w.conn, &w.t1_as_teacher(), &ListQuery { status: Some("published".into()), ..Default::default() }, NOW).unwrap().items;
                        let mine = r.iter().find(|x| x.info.id == id).unwrap();
                        assert_eq!((mine.visible, mine.hidden_reason), (expected.is_none(), expected), "row: {case}");
                        assert_eq!(served.contains(&id), expected.is_none(), "served to students: {case}");
                        // the admin's exam does not depend on any teacher: only the subject and institution matter
                        let admin_expected = if !institution_on { Some(crate::platform_content::HiddenReason::InstitutionInactive) } else if !subject_on { Some(crate::platform_content::HiddenReason::SubjectInactive) } else { None };
                        let a = get_exam(&w.conn, &w.admin, &admin_exam, NOW).unwrap().row;
                        assert_eq!((a.visible, a.hidden_reason), (admin_expected.is_none(), admin_expected), "admin exam: {case}");
                        assert_eq!(served.contains(&admin_exam), admin_expected.is_none(), "admin exam served: {case}");
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 5 * 4 * 2 * 2);
        // drafts have no reason (they are not hidden, they are not published); a closed exam is shown to students
        // (their results), so it can be hidden like a published one
        w.conn.execute("UPDATE users SET status = 'active', role = 'teacher' WHERE id = ?1", params![w.t1.id]).unwrap();
        w.conn.execute("INSERT OR REPLACE INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1','approved',0)", params![w.t1.id]).unwrap();
        w.conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        w.conn.execute("UPDATE assessments SET status = 'draft' WHERE id = ?1", params![id]).unwrap();
        assert_eq!((row(&w, &w.admin, &id).visible, row(&w, &w.admin, &id).hidden_reason), (false, None));
        w.conn.execute("UPDATE assessments SET status = 'closed' WHERE id = ?1", params![id]).unwrap();
        assert_eq!(row(&w, &w.admin, &id).hidden_reason, Some(crate::platform_content::HiddenReason::InstitutionInactive));
    }

    impl W {
        /// The teacher as the engine sees the signed-in user (role / id never come from the database row).
        fn t1_as_teacher(&self) -> User {
            User { role: "teacher".into(), status: "active".into(), ..self.t1.clone() }
        }
    }

    /// The wire shapes the frontend codes against (contract A and D): a change here is a breaking change.
    #[test]
    fn the_json_shapes_are_the_agreed_contract() {
        let w = world();
        let id = mk(&w, &w.t1, json!({"status": "published"}));
        add_attempt(&w, &id);
        let keys = |v: &Value| -> std::collections::BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
        let detail = serde_json::to_value(get_exam(&w.conn, &w.t1, &id, NOW).unwrap()).unwrap();
        let expected: std::collections::BTreeSet<String> = [
            // the exam (AssessmentInfo)
            "id", "teacher_id", "teacher_name", "subject_id", "subject_name", "title", "description", "question_count", "total_points", "duration_min", "opens_at", "closes_at",
            "max_attempts", "show_answers", "status", "attempts_used", "attempt_count", "shuffle_questions", "shuffle_options", "pass_mark", "release_mode", "closed_at", "archived_at", "created_by",
            // what the builder adds
            "phase", "can_edit", "owned", "locked", "submitted", "pending_answers", "visible", "hidden_reason", "questions", "sources",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(keys(&detail), expected);
        assert_eq!((detail["owned"].clone(), detail["locked"].clone(), detail["submitted"].clone(), detail["pending_answers"].clone(), detail["hidden_reason"].clone()), (json!(true), json!(false), json!(1), json!(0), Value::Null));
        let page = serde_json::to_value(list_exams(&w.conn, &w.t1, &ListQuery::default(), NOW).unwrap()).unwrap();
        assert_eq!(keys(&page), ["items", "total"].into_iter().map(String::from).collect());
        let mut row_keys = keys(&page["items"][0]);
        assert!(row_keys.remove("questions") == false && row_keys.remove("sources") == false, "list rows carry no questions");
        assert_eq!(row_keys.len(), expected.len() - 2);
        // hidden reasons are the snake_case names of /assessments/mine
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(serde_json::to_value(row(&w, &w.t1, &id)).unwrap()["hidden_reason"], json!("institution_inactive"));
    }

    #[test]
    fn routes_register_without_conflicts() {
        // Router::route panics when two registrations collide; /{id}/{action} and /{id}/questions/{qid}/correct share a prefix.
        // (Which handler answers each shape is asserted over real requests in `http_tests` below.)
        let _ = routes();
    }

    #[test]
    fn exam_requests_deserialize_the_documented_fields() {
        let r: ExamReq = serde_json::from_value(json!({
            "subject_id": "s1", "title": "ت", "description": "و", "questions": [], "sources": {"q": "b"}, "duration_min": 5, "clear_duration": false,
            "opens_at": 1, "clear_opens": false, "closes_at": 2, "clear_closes": false, "max_attempts": 2, "show_answers": true, "shuffle_questions": true,
            "shuffle_options": true, "pass_mark": 50.5, "clear_pass_mark": false, "release_mode": "immediate", "status": "draft"
        }))
        .unwrap();
        assert_eq!((r.max_attempts, r.pass_mark), (Some(2), Some(50.5)));
        let _: HashMap<String, String> = r.sources.unwrap();
    }
}

/// The exam routes over real requests (a `Router` driven with `oneshot`): which handler answers which URL shape, the
/// role wall on each route set, the body extractor, and the per-user limit of the answer-key correction.
#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, new_id, PlatformState};
    use crate::platform_exam_admin::{create_exam, ExamReq};
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request};
    use axum::routing::get;
    use rusqlite::params;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const NOW: i64 = 1_000_000_000_000;

    struct H {
        app: Router,
        state: Arc<AppState>,
        admin: (User, String),
        teacher: (User, String),
        other: (User, String),
        student: (User, String),
        exam: String,
    }

    fn token_for(conn: &Connection, u: &User) -> String {
        let token = format!("tok-{}", new_id());
        conn.execute(
            "INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, 0, ?3)",
            params![crate::relay::sha256_hex(&token), u.id, crate::relay::now_ms() + 3_600_000],
        )
        .unwrap();
        token
    }

    fn harness() -> H {
        // `ConfigStore` keeps its key under the OS config dir: point that at a scratch directory for the test process
        static CFG: std::sync::Once = std::sync::Once::new();
        CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-http-test-{}", new_id()))));
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        for t in [&teacher, &other] {
            conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![t.id]).unwrap();
        }
        let r: ExamReq = serde_json::from_value(json!({
            "subject_id": "s1", "title": "امتحان", "status": "published", "closes_at": crate::relay::now_ms() + 3_600_000,
            "questions": [{"id": "q1", "type": "single_choice", "stem": "س", "options": ["a", "b", "c"], "answer": "B", "score": 1}]
        }))
        .unwrap();
        let exam = create_exam(&conn, &teacher, &r, crate::relay::now_ms()).unwrap().info_id();
        let tokens: Vec<String> = [&admin, &teacher, &other, &student].iter().map(|u| token_for(&conn, u)).collect();
        let state = Arc::new(AppState {
            config_store: exameow_core::config::ConfigStore::new("ExameowServerHttpTest").expect("a scratch config dir"),
            relay: crate::relay::init_db(":memory:").unwrap(),
            platform: PlatformState::for_tests(conn),
            admin_token: std::sync::Mutex::new("pass".into()),
            legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
            auth_failures: crate::token_limit::TokenLimiter::new(600),
        });
        let mut t = tokens.into_iter();
        H {
            app: routes().with_state(state.clone()),
            state,
            admin: (admin, t.next().unwrap()),
            teacher: (teacher, t.next().unwrap()),
            other: (other, t.next().unwrap()),
            student: (student, t.next().unwrap()),
            exam,
        }
    }

    /// One request: `(status, parsed JSON body or Null)`.
    async fn call(h: &H, method: Method, uri: &str, token: Option<&str>, body: Option<Body>, json_type: bool) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if json_type {
            b = b.header(header::CONTENT_TYPE, "application/json");
        }
        let res = h.app.clone().oneshot(b.body(body.unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(h: &H, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        call(h, Method::POST, uri, token, Some(Body::from(body.to_string())), true).await
    }

    fn code(v: &Value) -> &str {
        v["error"].as_str().unwrap_or("")
    }

    /// `route` = (verb, path under `/api/platform/<set>/`, optional JSON body).
    async fn send(h: &H, set: &str, route: &(Method, String, Option<Value>), token: Option<String>) -> (StatusCode, Value) {
        let (m, path, body) = route;
        call(h, m.clone(), &format!("/api/platform/{set}/{path}"), token.as_deref(), body.as_ref().map(|b| Body::from(b.to_string())), true).await
    }

    #[tokio::test]
    async fn each_url_shape_reaches_its_own_handler_for_both_route_sets() {
        let h = harness();
        let id = &h.exam;
        for (set, token) in [("teacher", &h.teacher.1), ("admin", &h.admin.1)] {
            // /{id}/{action}: the lifecycle handler (an unknown action is ITS 400, an unknown exam ITS 404)
            let (st, v) = post(&h, &format!("/api/platform/{set}/exams/{id}/explode"), Some(token), json!({})).await;
            assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_action"), "{set}: the action handler");
            // /{id}/questions/{qid}/correct: the correction handler — it validates the BODY, which the action handler never does
            let (st, v) = post(&h, &format!("/api/platform/{set}/exams/{id}/questions/q1/correct"), Some(token), json!({"mode": "set", "answer": "C", "dry_run": true})).await;
            if set == "teacher" {
                assert_eq!(st, StatusCode::OK, "{set}: {v}");
                assert_eq!((v["dry_run"].clone(), v["question"]["answer"].clone(), v["mode"].clone()), (json!(true), json!("C"), json!("set")), "{set}: a correction result, not a lifecycle detail");
            } else {
                assert_eq!((st, code(&v)), (StatusCode::FORBIDDEN, "forbidden"), "{set}: the correction handler (an admin does not own a teacher's exam)");
            }
            let (st, v) = post(&h, &format!("/api/platform/{set}/exams/{id}/questions/nope/correct"), Some(token), json!({"mode": "void"})).await;
            let expected = if set == "teacher" { (StatusCode::NOT_FOUND, "not_found") } else { (StatusCode::FORBIDDEN, "forbidden") };
            assert_eq!((st, code(&v)), expected, "{set}: an unknown question is the correction handler's answer, never `invalid_action`");
            // an id with an escaped slash still reaches the correction handler as ONE segment
            let (st, v) = post(&h, &format!("/api/platform/{set}/exams/{id}/questions/q%2F9/correct"), Some(token), json!({"mode": "void"})).await;
            assert_eq!(code(&v) != "invalid_action", true, "{set}: {st} {v}");
        }
        // and a real action goes through the action handler: the owner closes their exam
        let (st, v) = post(&h, &format!("/api/platform/teacher/exams/{id}/close"), Some(&h.teacher.1), json!({})).await;
        assert_eq!((st, v["status"].as_str(), v["locked"].clone()), (StatusCode::OK, Some("closed"), json!(false)));
    }

    #[tokio::test]
    async fn the_role_wall_holds_on_every_route_of_both_sets() {
        let h = harness();
        let id = h.exam.clone();
        let routes: Vec<(Method, String, Option<Value>)> = vec![
            (Method::GET, "exams".into(), None),
            (Method::POST, "exams".into(), Some(json!({}))),
            (Method::GET, format!("exams/{id}"), None),
            (Method::PATCH, format!("exams/{id}"), Some(json!({"title": "x"}))),
            (Method::DELETE, format!("exams/{id}"), None),
            (Method::POST, format!("exams/{id}/close"), Some(json!({}))),
            (Method::POST, format!("exams/{id}/questions/q1/correct"), Some(json!({"mode": "void"}))),
        ];
        for r in &routes {
            // no session: 401 on both sets
            for set in ["teacher", "admin"] {
                let (st, v) = send(&h, set, r, None).await;
                assert_eq!((st, code(&v)), (StatusCode::UNAUTHORIZED, "unauthorized"), "{set} {} {} without a token", r.0, r.1);
            }
            // students and the wrong staff role: 403 — and nothing was changed by any of it
            for (who, tok, set) in [("student", &h.student.1, "teacher"), ("student", &h.student.1, "admin"), ("admin", &h.admin.1, "teacher"), ("teacher", &h.teacher.1, "admin"), ("other teacher", &h.other.1, "admin")] {
                let (st, v) = send(&h, set, r, Some(tok.clone())).await;
                assert_eq!((st, code(&v)), (StatusCode::FORBIDDEN, "forbidden"), "{who} on {set} {} {}", r.0, r.1);
            }
        }
        let still: (String, String) = h.state.platform.conn.lock().unwrap().query_row("SELECT status, title FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(still, ("published".to_string(), "امتحان".to_string()), "no refused request changed the exam");
        // the right role, the wrong owner: 403 on the teacher routes, whatever the verb
        for r in routes.iter().skip(2) {
            let (st, v) = send(&h, "teacher", r, Some(h.other.1.clone())).await;
            assert_eq!((st, code(&v)), (StatusCode::FORBIDDEN, "forbidden"), "another teacher {} {}", r.0, r.1);
        }
        let (st, v) = call(&h, Method::GET, "/api/platform/teacher/exams/ghost", Some(&h.teacher.1), None, false).await;
        assert_eq!((st, code(&v)), (StatusCode::NOT_FOUND, "not_found"));
        // suspended accounts are not "active" teachers any more
        h.state.platform.conn.lock().unwrap().execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![h.teacher.0.id]).unwrap();
        let (st, _) = call(&h, Method::GET, "/api/platform/teacher/exams", Some(&h.teacher.1), None, false).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_exam_page_tells_staff_whether_an_exam_is_locked_and_students_nothing() {
        let h = harness();
        let detail = |h: &H, token: String| {
            let app = Router::new().route("/api/platform/assessments/{id}", get(crate::platform_exams::detail_handler)).with_state(h.state.clone());
            let uri = format!("/api/platform/assessments/{}", h.exam);
            async move {
                let res = app.oneshot(Request::builder().uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
                let status = res.status();
                (status, serde_json::from_slice::<Value>(&to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap_or(Value::Null))
            }
        };
        h.state.platform.conn.lock().unwrap().execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![h.student.0.id]).unwrap();
        for who in [&h.admin.1, &h.teacher.1, &h.student.1] {
            let (st, v) = detail(&h, who.clone()).await;
            assert_eq!((st, v["locked"].clone()), (StatusCode::OK, json!(false)), "a live exam is not locked");
        }
        // an admin closes the teacher's exam: staff see `locked`, a student sees the field but never `true`
        let (st, _) = post(&h, &format!("/api/platform/admin/exams/{}/close", h.exam), Some(&h.admin.1), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        for (who, token, expected) in [("admin", &h.admin.1, true), ("owner", &h.teacher.1, true), ("student", &h.student.1, false)] {
            let (st, v) = detail(&h, token.clone()).await;
            assert_eq!((st, v["locked"].clone()), (StatusCode::OK, json!(expected)), "{who}");
        }
        // and the admin's unlock over HTTP hands it back
        let (st, v) = post(&h, &format!("/api/platform/admin/exams/{}/unlock", h.exam), Some(&h.admin.1), json!({})).await;
        assert_eq!((st, v["locked"].clone(), v["status"].clone()), (StatusCode::OK, json!(false), json!("closed")));
        let (_, v) = detail(&h, h.teacher.1.clone()).await;
        assert_eq!(v["locked"], json!(false));
    }

    #[tokio::test]
    async fn a_bad_correction_body_is_rejected_before_anything_runs() {
        let h = harness();
        let uri = format!("/api/platform/teacher/exams/{}/questions/q1/correct", h.exam);
        let tok = Some(h.teacher.1.as_str());
        // not JSON, JSON of the wrong shape, no content type, a wrong field type
        let (st, _) = call(&h, Method::POST, &uri, tok, Some(Body::from("{not json")), true).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = call(&h, Method::POST, &uri, tok, Some(Body::from(r#"{"mode":"set","score":"many"}"#)), true).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (st, _) = call(&h, Method::POST, &uri, tok, Some(Body::from(r#"{"mode":5}"#)), true).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (st, _) = call(&h, Method::POST, &uri, tok, Some(Body::from(r#"{"mode":"void"}"#)), false).await;
        assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        // a valid body the engine refuses carries the engine's code
        let (st, v) = post(&h, &uri, tok, json!({"mode": "set"})).await;
        assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_question"));
        let (st, v) = post(&h, &uri, tok, json!({"mode": "set", "score": 101})).await;
        assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_points"));
        let (st, v) = post(&h, &uri, tok, json!({"mode": "everything"})).await;
        assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_mode"));
        let (st, v) = post(&h, &uri, tok, json!({"mode": "set", "answer": "Z"})).await;
        assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_answer"));
        // and the key is untouched by all of it
        let ans: String = h.state.platform.conn.lock().unwrap().query_row("SELECT questions FROM assessments WHERE id = ?1", params![h.exam], |r| r.get(0)).unwrap();
        assert!(ans.contains("\"answer\":\"B\""), "{ans}");
    }

    #[tokio::test]
    async fn corrections_are_limited_to_thirty_a_minute_per_person_and_other_people_are_not_affected() {
        let h = harness();
        let uri = format!("/api/platform/teacher/exams/{}/questions/q1/correct", h.exam);
        let dry = || json!({"mode": "set", "answer": "C", "dry_run": true});
        // a window boundary in the middle of the run would reset the count: retry in the next window if one passed
        for _attempt in 0..3 {
            let bucket = || crate::relay::now_ms() / 60_000;
            let b0 = bucket();
            let mut limited_at = None;
            for n in 1..=32 {
                let (st, v) = post(&h, &uri, Some(&h.teacher.1), dry()).await;
                if st == StatusCode::TOO_MANY_REQUESTS {
                    assert_eq!(code(&v), "rate_limited");
                    limited_at = Some(n);
                    break;
                }
                assert_eq!(st, StatusCode::OK, "call {n}: {v}");
            }
            if bucket() != b0 {
                h.state.platform.conn.lock().unwrap().execute("DELETE FROM rate_limits", []).unwrap();
                continue;
            }
            assert_eq!(limited_at, Some(31), "30 are allowed in a minute, the 31st is refused");
            // the limit is per user: the admin's own (admin-route) calls on an admin exam are counted apart
            let (st, _) = post(&h, &uri, Some(&h.teacher.1), dry()).await;
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "still limited");
            let (st, v) = post(&h, &format!("/api/platform/admin/exams/{}/questions/q1/correct", h.exam), Some(&h.admin.1), dry()).await;
            assert_eq!((st, code(&v)), (StatusCode::FORBIDDEN, "forbidden"), "another person is not limited by the teacher's counter (and is refused for another reason)");
            return;
        }
        panic!("the minute window kept changing under the test");
    }
}
