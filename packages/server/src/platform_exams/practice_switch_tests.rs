//! Phase 4-3, the practice switch (`assessments.allow_practice`): on create, on update, on duplicate, in rows and details, for
//! the admin door, the teacher door and the legacy routes, and the migration that adds the column.

use super::fixtures::*;
use super::*;
use crate::platform_exam_admin::{act, duplicate, get_exam, list_exams, update_exam, ExamReq, ListQuery};
use crate::platform_exam_teacher::lifecycle_checked;
use crate::platform_settings::Crypto;
use serde_json::{json, Value};

fn req(v: Value) -> ExamReq {
    serde_json::from_value(v).unwrap()
}

fn practice_of(w: &W, who: &User, id: &str) -> bool {
    get_exam(&w.conn, who, id, NOW).unwrap().row.info.allow_practice
}

fn changes(w: &W, id: &str) -> Vec<Value> {
    w.conn
        .prepare("SELECT detail FROM audit_log WHERE action = 'exam_practice_changed' AND target_id = ?1 ORDER BY id")
        .unwrap()
        .query_map(params![id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|d| serde_json::from_str(&d.unwrap()).unwrap())
        .collect()
}

#[test]
fn practice_is_off_on_create_unless_asked_for_and_the_request_is_optional_on_update() {
    let w = world();
    let off = exam(&w, json!({}));
    let on = exam(&w, json!({"title": "مفعّل", "allow_practice": true}));
    assert!(!practice_of(&w, &w.admin, &off) && practice_of(&w, &w.admin, &on));
    // absent = unchanged
    update_exam(&w.conn, &w.admin, &on, &req(json!({"title": "عنوان"})), NOW + 1).unwrap();
    assert!(practice_of(&w, &w.admin, &on), "an update that does not mention it leaves it on");
    update_exam(&w.conn, &w.admin, &off, &req(json!({"title": "عنوان"})), NOW + 1).unwrap();
    assert!(!practice_of(&w, &w.admin, &off));
    // explicit values
    assert!(update_exam(&w.conn, &w.admin, &off, &req(json!({"allow_practice": true})), NOW + 2).unwrap().row.info.allow_practice);
    assert!(!update_exam(&w.conn, &w.admin, &off, &req(json!({"allow_practice": false})), NOW + 3).unwrap().row.info.allow_practice);
}

#[test]
fn every_real_change_is_audited_once_and_a_save_that_changes_nothing_is_not() {
    let w = world();
    let id = exam(&w, json!({}));
    assert!(changes(&w, &id).is_empty(), "created off: no change recorded");
    update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": false})), NOW + 1).unwrap();
    assert!(changes(&w, &id).is_empty(), "off -> off is not a change");
    update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": true})), NOW + 2).unwrap();
    update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": true, "title": "ب"})), NOW + 3).unwrap();
    update_exam(&w.conn, &w.admin, &id, &req(json!({"title": "ج"})), NOW + 4).unwrap();
    update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": false})), NOW + 5).unwrap();
    assert_eq!(
        changes(&w, &id),
        vec![json!({"exam_id": id, "from": false, "to": true}), json!({"exam_id": id, "from": true, "to": false})],
        "exactly the two real changes, with who/what in the row"
    );
    let actor: String = w.conn.query_row("SELECT actor_id FROM audit_log WHERE action = 'exam_practice_changed' LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(actor, w.admin.id);
    // switching it on at creation is recorded too (the answers become visible to students from the first minute)
    let born_on = exam(&w, json!({"title": "مفعّل", "allow_practice": true}));
    assert_eq!(changes(&w, &born_on), vec![json!({"exam_id": born_on, "from": false, "to": true})]);
}

#[test]
fn the_switch_is_not_frozen_by_attempts_while_the_questions_still_are() {
    let w = world();
    let id = exam(&w, json!({}));
    take(&w, &w.student, &id, NOW, MIN, &objective_right());
    assert_eq!(fail(update_exam(&w.conn, &w.admin, &id, &req(json!({"questions": [{"id":"z","type":"fill_blank","stem":"س","answer":"x"}]})), NOW + 1)).1, "has_attempts");
    assert!(update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": true})), NOW + 2).unwrap().row.info.allow_practice, "an exam somebody sat can still allow practice");
    assert!(!update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": false})), NOW + 3).unwrap().row.info.allow_practice);
    // `check_freeze` does not list it: the freeze compares settings, and this is not one
    let base = Cfg::defaults();
    assert!(check_freeze(&base, &base, false).is_ok());
    // …and it can be flipped on an exam that is closed (not locked) as well
    act(&w.conn, &w.admin, &id, "close", NOW + 4).unwrap();
    assert!(update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": true})), NOW + 5).unwrap().row.info.allow_practice);
}

#[test]
fn a_copy_never_inherits_the_switch() {
    let w = world();
    let id = exam(&w, json!({"allow_practice": true}));
    let copy = duplicate(&w.conn, &w.admin, &id, NOW + 1).unwrap();
    assert!(!copy.row.info.allow_practice, "the answers are not shown by a copy the owner has not looked at");
    assert!(practice_of(&w, &w.admin, &id), "the original is untouched");
}

#[test]
fn rows_details_and_lists_carry_the_switch_on_the_wire() {
    let w = world();
    let on = exam(&w, json!({"allow_practice": true}));
    let off = exam(&w, json!({"title": "مغلق"}));
    let detail = serde_json::to_value(get_exam(&w.conn, &w.admin, &on, NOW).unwrap()).unwrap();
    assert_eq!(detail["allow_practice"], json!(true));
    let page = list_exams(&w.conn, &w.admin, &ListQuery::default(), NOW).unwrap();
    let flags: HashMap<String, bool> = page.items.iter().map(|r| (r.info.id.clone(), r.info.allow_practice)).collect();
    assert_eq!((flags[&on], flags[&off]), (true, false));
    assert_eq!(serde_json::to_value(&page.items[0]).unwrap()["allow_practice"].is_boolean(), true);
    // the older owner list (`GET /assessments/mine`) and a student's exam page carry it too
    let theirs = exam_by(&w, &w.teacher, json!({"allow_practice": true}));
    let mine = list_mine(&w.conn, &w.teacher.id).unwrap();
    assert_eq!(serde_json::to_value(&mine[0]).unwrap()["allow_practice"], json!(true));
    let info = get_info(&w.conn, &w.student.id, &theirs).unwrap();
    assert!(info.allow_practice);
}

// ───────── the teacher's door ─────────

#[test]
fn a_teacher_switches_their_own_exam_at_any_time_and_nobody_else_can() {
    let w = world();
    let crypto = Crypto::for_tests();
    let id = exam_by(&w, &w.teacher, json!({}));
    let other = crate::platform::insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
    w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![other.id]).unwrap();
    assert!(update_exam(&w.conn, &w.teacher, &id, &req(json!({"allow_practice": true})), NOW + 1).unwrap().row.info.allow_practice);
    // a colleague and the admin only moderate: they never rewrite settings
    assert_eq!(fail(update_exam(&w.conn, &other, &id, &req(json!({"allow_practice": false})), NOW + 2)).0, 403);
    assert_eq!(fail(update_exam(&w.conn, &w.admin, &id, &req(json!({"allow_practice": false})), NOW + 2)).0, 403);
    assert!(practice_of(&w, &w.admin, &id), "unchanged");
    // sat, then closed by the teacher themself: still theirs to change
    take(&w, &w.student, &id, NOW, MIN, &objective_right());
    lifecycle_checked(&w.conn, &crypto, &w.teacher, &id, "close", NOW + 3).unwrap();
    assert!(!update_exam(&w.conn, &w.teacher, &id, &req(json!({"allow_practice": false})), NOW + 4).unwrap().row.info.allow_practice);
    assert_eq!(changes(&w, &id).len(), 2);
    let actor: String = w.conn.query_row("SELECT actor_id FROM audit_log WHERE action = 'exam_practice_changed' LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(actor, w.teacher.id, "the audit row names the teacher");
}

#[test]
fn a_teacher_cannot_change_it_while_an_admin_has_the_exam_locked() {
    let w = world();
    let id = exam_by(&w, &w.teacher, json!({"allow_practice": true}));
    take(&w, &w.student, &id, NOW, MIN, &objective_right());
    act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
    assert_eq!(fail(update_exam(&w.conn, &w.teacher, &id, &req(json!({"allow_practice": false})), NOW + 2)), (409, "locked".into()));
    assert!(practice_of(&w, &w.admin, &id), "still on: the admin's close stands");
    act(&w.conn, &w.admin, &id, "unlock", NOW + 3).unwrap();
    assert!(!update_exam(&w.conn, &w.teacher, &id, &req(json!({"allow_practice": false})), NOW + 4).unwrap().row.info.allow_practice, "handed back: theirs again");
}

// ───────── the legacy routes ─────────

fn legacy(v: Value) -> AssessmentReq {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_legacy_routes_carry_the_switch_for_the_owner_only() {
    let w = world();
    let r = legacy(json!({"subject_id": "s1", "title": "قديم", "questions": questions(), "status": "published", "allow_practice": true}));
    let created = create_assessment(&w.conn, &w.teacher, &r).unwrap();
    assert!(created.allow_practice);
    assert_eq!(changes(&w, &created.id).len(), 1);
    let plain = create_assessment(&w.conn, &w.teacher, &legacy(json!({"subject_id": "s1", "title": "عادي", "questions": questions()}))).unwrap();
    assert!(!plain.allow_practice, "off unless asked");
    // the owner changes it even after students sat (it is not frozen)…
    take(&w, &w.student, &created.id, NOW, MIN, &objective_right());
    let off = update_assessment(&w.conn, &w.teacher, &created.id, &legacy(json!({"allow_practice": false}))).unwrap();
    assert!(!off.allow_practice);
    assert_eq!(update_assessment(&w.conn, &w.teacher, &created.id, &legacy(json!({"title": "عنوان آخر"}))).unwrap().allow_practice, false, "absent = unchanged");
    // …and an admin moderating the exam through this route may not touch it
    assert_eq!(fail(update_assessment(&w.conn, &w.admin, &created.id, &legacy(json!({"allow_practice": true}))).map(|_| ())), (403, "forbidden".into()));
    assert_eq!(changes(&w, &created.id).len(), 2, "on at creation, off by the owner; the refused attempt wrote nothing");
}

// ───────── the migration ─────────

mod migration {
    use super::super::*;
    use crate::platform::{apply_schema, column_exists, create_test_db};

    const FIXTURE: &str = include_str!("../../tests/fixtures/platform_v1.sql");

    fn old_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        c.execute_batch(FIXTURE).unwrap();
        c
    }

    fn column(c: &Connection) -> (String, i64, Option<String>) {
        c.query_row("SELECT type, \"notnull\", dflt_value FROM pragma_table_info('assessments') WHERE name = 'allow_practice'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
    }

    #[test]
    fn the_pre_1_0_database_gains_the_column_off_for_every_exam_and_keeps_everything() {
        let c = old_db();
        assert!(!column_exists(&c, "assessments", "allow_practice").unwrap());
        let before: i64 = c.query_row("SELECT count(*) FROM assessments", [], |r| r.get(0)).unwrap();
        let scores: String = c.query_row("SELECT group_concat(id || ':' || score, ',') FROM (SELECT * FROM attempts ORDER BY id)", [], |r| r.get(0)).unwrap();
        apply_schema(&c).unwrap();
        assert_eq!(column(&c), ("INTEGER".to_string(), 1, Some("0".to_string())));
        assert_eq!(c.query_row::<i64, _, _>("SELECT count(*) FROM assessments", [], |r| r.get(0)).unwrap(), before);
        assert_eq!(c.query_row::<i64, _, _>("SELECT count(*) FROM assessments WHERE allow_practice <> 0", [], |r| r.get(0)).unwrap(), 0, "practice is off for every exam from before");
        let after: String = c.query_row("SELECT group_concat(id || ':' || score, ',') FROM (SELECT * FROM attempts ORDER BY id)", [], |r| r.get(0)).unwrap();
        assert_eq!(after, scores, "attempts untouched");
        assert_eq!(c.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0)).unwrap(), "ok");
        // idempotent
        apply_schema(&c).unwrap();
        apply_schema(&c).unwrap();
        assert_eq!(column(&c), ("INTEGER".to_string(), 1, Some("0".to_string())));
    }

    #[test]
    fn a_database_that_already_has_the_newer_columns_gets_it_by_a_plain_alter() {
        let c = create_test_db();
        c.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i','university','ج',0)", []).unwrap();
        c.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s','i','م',0)", []).unwrap();
        c.execute("INSERT INTO assessments(id, subject_id, title, questions, question_count, total_points, status, created_at, updated_at) VALUES ('e','s','ت','[]',0,0,'draft',0,0)", []).unwrap();
        // a database of the previous version: the column is not there yet
        c.execute_batch("ALTER TABLE assessments DROP COLUMN allow_practice").unwrap();
        assert!(!column_exists(&c, "assessments", "allow_practice").unwrap());
        apply_schema(&c).unwrap();
        assert_eq!(column(&c), ("INTEGER".to_string(), 1, Some("0".to_string())));
        assert_eq!(c.query_row::<i64, _, _>("SELECT allow_practice FROM assessments WHERE id = 'e'", [], |r| r.get(0)).unwrap(), 0, "the existing exam is off");
        c.execute("UPDATE assessments SET allow_practice = 1 WHERE id = 'e'", []).unwrap();
        apply_schema(&c).unwrap();
        assert_eq!(c.query_row::<i64, _, _>("SELECT allow_practice FROM assessments WHERE id = 'e'", [], |r| r.get(0)).unwrap(), 1, "running it again keeps what was set");
    }
}
