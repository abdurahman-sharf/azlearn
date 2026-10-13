//! Test support shared by the phase 4-3 test modules (hard end, partial autosave and review, practice, statistics, HTTP): one
//! institution with one subject, an admin, a teacher approved for it and an enrolled student, plus helpers that build exams
//! through the real builder (`platform_exam_admin::create_exam`) so every row is exactly what the product writes.

use super::*;
use crate::platform::{create_test_db, insert_test_user};
use crate::platform_exam_admin::{create_exam, ExamReq};
use serde_json::{json, Value};

pub(crate) const NOW: i64 = 1_000_000_000_000;
pub(crate) const MIN: i64 = 60_000;
pub(crate) const H: i64 = 3_600_000;

pub(crate) struct W {
    pub conn: Connection,
    pub admin: User,
    /// Approved for `s1`.
    pub teacher: User,
    /// Enrolled in `s1`.
    pub student: User,
}

/// Institution `i1`, subjects `s1` and `s2`, the admin, a teacher approved for `s1`, and a student enrolled in `s1`.
pub(crate) fn world() -> W {
    let conn = create_test_db();
    conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
    conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
    conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s2','i1','رياضيات',0)", []).unwrap();
    let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
    let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
    let student = insert_test_user(&conn, "s@x.com", "student", "active");
    conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
    conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![student.id]).unwrap();
    W { conn, admin, teacher, student }
}

/// A new active student enrolled in `s1`.
pub(crate) fn enroll(w: &W, email: &str) -> User {
    let u = insert_test_user(&w.conn, email, "student", "active");
    w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![u.id]).unwrap();
    u
}

/// Five questions worth 9 points: a single choice (B), a multiple choice (A and C), a true/false (A), a fill-in
/// (`القاهرة|مصر`) and a written answer (4 points). Option texts are `o<q>-<index>`; chapters `ف1` (q1, q2) and `ف2` (q3-q5).
pub(crate) fn questions() -> Value {
    let opts = |q: usize, n: usize| (0..n).map(|i| format!("o{q}-{i}")).collect::<Vec<_>>();
    json!([
        {"id":"q1","type":"single_choice","stem":"س1","options":opts(1,4),"answer":"B","analysis":"شرح 1","score":1,"chapter":"ف1","difficulty":"easy"},
        {"id":"q2","type":"multi_choice","stem":"س2","options":opts(2,4),"answer":"AC","analysis":"","score":2,"chapter":"ف1","difficulty":"medium"},
        {"id":"q3","type":"true_false","stem":"س3","options":["صحيح","خطأ"],"answer":"A","score":1,"chapter":"ف2","difficulty":"hard"},
        {"id":"q4","type":"fill_blank","stem":"س4","answer":"القاهرة|مصر","score":1,"chapter":"ف2"},
        {"id":"q5","type":"short_answer","stem":"س5","answer":"نموذج","analysis":"الإجابة النموذجية","score":4,"chapter":"ف2"}
    ])
}

/// The exam request with `extra` merged over the defaults (published, three attempts, [`questions`]).
pub(crate) fn exam_req(extra: Value) -> ExamReq {
    let mut v = json!({"subject_id":"s1","title":"امتحان","questions":questions(),"status":"published","max_attempts":3});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

/// Creates an exam as `owner` (the admin makes an admin exam, the teacher their own) at [`NOW`]; returns its id.
pub(crate) fn exam_by(w: &W, owner: &User, extra: Value) -> String {
    create_exam(&w.conn, owner, &exam_req(extra), NOW).unwrap().info_id()
}

/// An admin exam (no owning teacher).
pub(crate) fn exam(w: &W, extra: Value) -> String {
    exam_by(w, &w.admin, extra)
}

pub(crate) fn ans(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// All five questions answered right (the written one with some text).
pub(crate) fn all_right() -> HashMap<String, String> {
    ans(&[("q1", "B"), ("q2", "AC"), ("q3", "A"), ("q4", "مصر"), ("q5", "إجابتي")])
}

/// Only the four objective questions right: 5 of 9, and the written answer left blank (so nothing waits for a grade).
pub(crate) fn objective_right() -> HashMap<String, String> {
    ans(&[("q1", "B"), ("q2", "AC"), ("q3", "A"), ("q4", "مصر")])
}

/// Starts as `who` at `at` and submits `answers` at `at + after`; returns the attempt id.
pub(crate) fn take(w: &W, who: &User, exam: &str, at: i64, after: i64, answers: &HashMap<String, String>) -> String {
    let s = start_attempt(&w.conn, who, exam, at).unwrap();
    submit_attempt(&w.conn, who, &s.attempt_id, answers, at + after).unwrap();
    s.attempt_id
}

pub(crate) fn status_of(w: &W, attempt: &str) -> String {
    w.conn.query_row("SELECT status FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap()
}

pub(crate) fn submitted_at(w: &W, attempt: &str) -> Option<i64> {
    w.conn.query_row("SELECT submitted_at FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap()
}

pub(crate) fn score_of(w: &W, attempt: &str) -> f64 {
    w.conn.query_row("SELECT score FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap()
}

/// Rows of a table (or any `count(*)` query).
pub(crate) fn count(w: &W, sql: &str) -> i64 {
    w.conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// The `error` code of a failed call, with its status.
pub(crate) fn fail<T: std::fmt::Debug>(r: Res<T>) -> (u16, String) {
    let e = r.unwrap_err();
    (e.0.as_u16(), serde_json::from_str::<Value>(&e.1).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default())
}

/// Every row of every table, as text: two equal dumps mean nothing was written between them.
pub(crate) fn dump_all(conn: &Connection) -> String {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = String::new();
    for t in tables {
        let mut st = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = st.column_count();
        let rows = st
            .query_map([], |r| {
                let mut s = format!("{t}|");
                for i in 0..n {
                    s += &format!("{:?}|", r.get::<_, rusqlite::types::Value>(i).unwrap());
                }
                Ok(s)
            })
            .unwrap();
        for r in rows {
            out += &r.unwrap();
            out.push('\n');
        }
    }
    out
}
