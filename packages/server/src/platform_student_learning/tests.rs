//! Unit tests of the student's learning screens: the learning list, a live session's page, the schedule and its calendar
//! file. The world and its seeding helpers are `pub(super)` so the HTTP-level tests next door (`http_tests`) reuse them.

use super::*;
use crate::platform::{create_test_db, insert_test_user};
use serde_json::json;

pub(super) const NOW: i64 = 1_800_000_000_000;
pub(super) const MIN: i64 = 60_000;
pub(super) const H: i64 = 3_600_000;
pub(super) const DAY: i64 = 86_400_000;
/// Every link a student who is not enrolled must never receive carries one of these markers (the host of a join / lesson
/// link, and the video id of the YouTube lesson, which becomes an embed link).
pub(super) const LEAKS: [&str; 2] = ["secret.example", "dQw4w9WgXcQ"];

pub(super) fn assert_no_leak(body: &str, what: &str) {
    for marker in LEAKS {
        assert!(!body.contains(marker), "{what}: a withheld link ({marker}) is in the response: {body}");
    }
}

pub(super) struct W {
    pub conn: Connection,
    /// Owns the content below; approved for s1.
    pub teacher: User,
    /// Approved for s1 as well (a colleague of the same class).
    pub colleague: User,
    /// Approved for s2 only.
    pub stranger: User,
    pub admin: User,
    /// A student enrolled in s1.
    pub me: User,
    /// A student enrolled nowhere, following nobody.
    pub outsider: User,
}

pub(super) fn world() -> W {
    let conn = create_test_db();
    conn.execute_batch(
        "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
         INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0), ('s2','i1','رياضيات',1,0);",
    )
    .unwrap();
    let user = |email: &str, role: &str, name: &str| {
        let u = insert_test_user(&conn, email, role, "active");
        conn.execute("UPDATE users SET full_name = ?2 WHERE id = ?1", params![u.id, name]).unwrap();
        User { full_name: name.into(), ..u }
    };
    let teacher = user("t@x.com", "teacher", "المعلم");
    let colleague = user("c@x.com", "teacher", "الزميل");
    let stranger = user("st@x.com", "teacher", "الغريب");
    let admin = user("a@x.com", "admin", "المدير");
    let me = user("me@x.com", "student", "أنا");
    let outsider = user("out@x.com", "student", "زائر");
    for (t, s) in [(&teacher, "s1"), (&colleague, "s1"), (&stranger, "s2")] {
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at,decided_at) VALUES (?1,?2,'approved',0,0)", params![t.id, s]).unwrap();
    }
    enroll(&conn, &me, "s1");
    W { conn, teacher, colleague, stranger, admin, me, outsider }
}

pub(super) fn enroll(conn: &Connection, who: &User, subject: &str) {
    conn.execute("INSERT OR IGNORE INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,?2,0)", params![who.id, subject]).unwrap();
}

pub(super) fn follow(conn: &Connection, student: &User, teacher: &User) {
    conn.execute("INSERT OR IGNORE INTO teacher_follows(student_id,teacher_id,created_at) VALUES (?1,?2,0)", params![student.id, teacher.id]).unwrap();
}

/// A course with `lessons` lessons (`<id>-l1`, `<id>-l2`, ...): the first is a YouTube lesson, the second a direct file, the
/// rest plain links — every kind of link a lesson can carry. Returns the lesson ids in order.
pub(super) fn course_in(conn: &Connection, id: &str, teacher: &User, subject: &str, status: &str, updated_at: i64, lessons: usize) -> Vec<String> {
    conn
        .execute(
            "INSERT INTO courses(id,teacher_id,subject_id,title,description,status,created_at,updated_at,published_at) VALUES (?1,?2,?3,?1,NULL,?4,0,?5,?6)",
            params![id, teacher.id, subject, status, updated_at, (status == "published").then_some(0i64)],
        )
        .unwrap();
    (1..=lessons)
        .map(|n| {
            let lid = format!("{id}-l{n}");
            let (url, embed, kind) = match n {
                1 => ("https://youtu.be/dQw4w9WgXcQ".to_string(), Some("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ".to_string()), "youtube"),
                2 => (format!("https://secret.example/{id}/video.mp4"), Some(format!("https://secret.example/{id}/video.mp4")), "file"),
                _ => (format!("https://secret.example/{id}/page{n}"), None, "link"),
            };
            conn
                .execute(
                    "INSERT INTO lessons(id,course_id,position,section,title,description,video_url,embed_url,video_kind,created_at) VALUES (?1,?2,?3,NULL,?1,NULL,?4,?5,?6,?3)",
                    params![lid, id, n as i64, url, embed, kind],
                )
                .unwrap();
            lid
        })
        .collect()
}

pub(super) fn course(w: &W, id: &str, teacher: &User, subject: &str, status: &str, updated_at: i64, lessons: usize) -> Vec<String> {
    course_in(&w.conn, id, teacher, subject, status, updated_at, lessons)
}

pub(super) fn live(w: &W, id: &str, teacher: &User, subject: &str, starts: i64, minutes: i64, status: &str) {
    live_in(&w.conn, id, teacher, subject, starts, minutes, status)
}

/// A published exam of `subject` owned by the world's teacher.
pub(super) fn exam(w: &W, id: &str, subject: &str, opens: Option<i64>, closes: Option<i64>) {
    exam_in(&w.conn, &w.teacher, id, subject, opens, closes)
}

pub(super) fn done(conn: &Connection, who: &User, lesson: &str, at: i64) {
    conn.execute("INSERT INTO lesson_progress(student_id,lesson_id,completed_at) VALUES (?1,?2,?3)", params![who.id, lesson, at]).unwrap();
}

/// A live session whose join link is `https://secret.example/join/<id>`.
pub(super) fn live_in(conn: &Connection, id: &str, teacher: &User, subject: &str, starts: i64, minutes: i64, status: &str) {
    conn
        .execute(
            "INSERT INTO live_sessions(id,teacher_id,subject_id,title,description,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,?3,?1,'وصف',?4,?5,?6,?7,0)",
            params![id, teacher.id, subject, starts, minutes, format!("https://secret.example/join/{id}"), status],
        )
        .unwrap();
}

/// A published exam of `subject` owned by `teacher`.
pub(super) fn exam_in(conn: &Connection, teacher: &User, id: &str, subject: &str, opens: Option<i64>, closes: Option<i64>) {
    conn
        .execute(
            "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,opens_at,closes_at,created_at,updated_at)
             VALUES (?1,?2,?2,?3,?1,'[]',1,10,'published',?4,?5,0,0)",
            params![id, teacher.id, subject, opens, closes],
        )
        .unwrap();
}

fn ids(p: &LearningPage) -> Vec<&str> {
    p.items.iter().map(|i| i.course_id.as_str()).collect()
}

fn page(w: &W, who: &User, q: LearningQuery) -> LearningPage {
    learning_page(&w.conn, who, &q).unwrap()
}

fn status_of<T>(r: Res<T>) -> StatusCode {
    r.err().expect("an error").0
}

// ───────── the learning list ─────────

#[test]
fn only_courses_a_student_can_open_are_listed() {
    let w = world();
    course(&w, "ok", &w.teacher, "s1", "published", 10, 2);
    course(&w, "draft", &w.teacher, "s1", "draft", 10, 2);
    course(&w, "empty", &w.teacher, "s1", "published", 10, 0);
    course(&w, "elsewhere", &w.stranger, "s2", "published", 10, 2);
    assert_eq!(ids(&page(&w, &w.me, LearningQuery::default())), ["ok"], "published, with lessons, in an enrolled subject: nothing else");
    assert!(page(&w, &w.outsider, LearningQuery::default()).items.is_empty(), "a student enrolled nowhere has no courses");
    assert_eq!(page(&w, &w.me, LearningQuery::default()).total, 1);

    // the one visibility rule: the teacher's approval, the subject and the institution each take the course away and give it back
    let listed = || ids(&page(&w, &w.me, LearningQuery::default())) == ["ok"];
    w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's1'", params![w.teacher.id]).unwrap();
    assert!(!listed(), "assignment withdrawn");
    w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1 AND subject_id = 's1'", params![w.teacher.id]).unwrap();
    assert!(listed());
    w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
    assert!(!listed(), "subject switched off");
    w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
    w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
    assert!(!listed(), "institution switched off");
    w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
    w.conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![w.teacher.id]).unwrap();
    assert!(!listed(), "teacher account not active");
    w.conn.execute("UPDATE users SET status = 'active' WHERE id = ?1", params![w.teacher.id]).unwrap();
    assert!(listed());
    // unenrolling takes it away too
    w.conn.execute("DELETE FROM subject_enrollments WHERE student_id = ?1", params![w.me.id]).unwrap();
    assert!(!listed());
}

#[test]
fn state_percent_and_the_next_lesson_follow_the_students_own_progress() {
    let w = world();
    let l = course(&w, "c1", &w.teacher, "s1", "published", 50, 3);
    let item = |w: &W| learning_items(&w.conn, &w.me.id, None, None).unwrap().remove(0);
    let i = item(&w);
    assert_eq!((i.state, i.total, i.completed, i.percent, i.next_lesson_id.clone(), i.last_at), (LearningState::NotStarted, 3, 0, 0.0, Some(l[0].clone()), None));
    assert_eq!((i.title.as_str(), i.subject_id.as_str(), i.subject_name.as_str(), i.teacher_id.as_str(), i.teacher_name.as_str(), i.updated_at), ("c1", "s1", "برمجة", w.teacher.id.as_str(), "المعلم", 50));
    done(&w.conn, &w.me, &l[0], 1_000);
    let i = item(&w);
    assert_eq!((i.state, i.completed, i.percent, i.next_lesson_id.clone(), i.last_at), (LearningState::InProgress, 1, 33.33, Some(l[1].clone()), Some(1_000)));
    // the first lesson NOT done, not the one after the last done: l1 and l3 done leaves l2
    done(&w.conn, &w.me, &l[2], 2_000);
    let i = item(&w);
    assert_eq!((i.completed, i.percent, i.next_lesson_id.clone(), i.last_at), (2, 66.67, Some(l[1].clone()), Some(2_000)));
    done(&w.conn, &w.me, &l[1], 3_000);
    let i = item(&w);
    assert_eq!((i.state, i.completed, i.percent, i.next_lesson_id.clone(), i.last_at), (LearningState::Completed, 3, 100.0, None, Some(3_000)));

    // another student's progress is not mine
    let other = insert_test_user(&w.conn, "o2@x.com", "student", "active");
    enroll(&w.conn, &other, "s1");
    let theirs = learning_items(&w.conn, &other.id, None, None).unwrap().remove(0);
    assert_eq!((theirs.state, theirs.completed, theirs.last_at), (LearningState::NotStarted, 0, None));
}

#[test]
fn next_lesson_follows_position_then_creation_time_then_id() {
    let w = world();
    let l = course(&w, "c1", &w.teacher, "s1", "published", 1, 3);
    // the teacher moved the third lesson to the front
    w.conn.execute("UPDATE lessons SET position = 0 WHERE id = ?1", params![l[2]]).unwrap();
    let i = learning_items(&w.conn, &w.me.id, None, None).unwrap().remove(0);
    assert_eq!(i.next_lesson_id, Some(l[2].clone()));
    // same position: the older lesson first, then the smaller id
    w.conn.execute("UPDATE lessons SET position = 5, created_at = 20 WHERE id = ?1", params![l[0]]).unwrap();
    w.conn.execute("UPDATE lessons SET position = 5, created_at = 10 WHERE id = ?1", params![l[1]]).unwrap();
    done(&w.conn, &w.me, &l[2], 1);
    assert_eq!(learning_items(&w.conn, &w.me.id, None, None).unwrap().remove(0).next_lesson_id, Some(l[1].clone()));
}

#[test]
fn the_all_list_runs_in_progress_then_not_started_then_completed_and_pages_deterministically() {
    let w = world();
    // in progress: latest activity first; ties by course id
    for (id, last) in [("ip-old", 5_000), ("ip-new", 9_000), ("ip-tie-b", 7_000), ("ip-tie-a", 7_000)] {
        let l = course(&w, id, &w.teacher, "s1", "published", 1, 2);
        done(&w.conn, &w.me, &l[0], last);
    }
    // not started: most recently updated first
    for (id, updated) in [("ns-old", 100), ("ns-new", 200)] {
        course(&w, id, &w.teacher, "s1", "published", updated, 1);
    }
    // completed: latest activity first
    for (id, last) in [("done-old", 3_000), ("done-new", 8_000)] {
        let l = course(&w, id, &w.teacher, "s1", "published", 1, 1);
        done(&w.conn, &w.me, &l[0], last);
    }
    let all = page(&w, &w.me, LearningQuery::default());
    assert_eq!(ids(&all), ["ip-new", "ip-tie-a", "ip-tie-b", "ip-old", "ns-new", "ns-old", "done-new", "done-old"]);
    assert_eq!((all.total, all.counts.clone()), (8, LearningCounts { in_progress: 4, not_started: 2, completed: 2 }));
    // pages never repeat or lose a course
    let mut seen: Vec<String> = Vec::new();
    for offset in (0..8).step_by(3) {
        let p = page(&w, &w.me, LearningQuery { limit: Some(3), offset: Some(offset), ..Default::default() });
        assert_eq!(p.total, 8);
        seen.extend(p.items.into_iter().map(|i| i.course_id));
    }
    assert_eq!(seen, ids(&all));
    assert!(page(&w, &w.me, LearningQuery { offset: Some(8), ..Default::default() }).items.is_empty(), "past the end: empty, not an error");
    // a state tab narrows the rows and the total, never the counts
    for (state, expect) in [("in_progress", 4), ("not_started", 2), ("completed", 2)] {
        let p = page(&w, &w.me, LearningQuery { state: Some(state.into()), ..Default::default() });
        assert_eq!((p.items.len() as i64, p.total), (expect, expect), "{state}");
        assert!(p.items.iter().all(|i| serde_json::to_value(i.state).unwrap() == json!(state)));
        assert_eq!(p.counts, LearningCounts { in_progress: 4, not_started: 2, completed: 2 });
    }
    assert_eq!(page(&w, &w.me, LearningQuery { state: Some("all".into()), ..Default::default() }).total, 8);
    assert_eq!(page(&w, &w.me, LearningQuery { state: Some("  ".into()), ..Default::default() }).total, 8, "blank = absent");
}

#[test]
fn subject_and_search_filters_narrow_the_rows_and_the_counts() {
    let w = world();
    enroll(&w.conn, &w.me, "s2");
    w.conn.execute("UPDATE courses SET title = title", []).unwrap();
    let a = course(&w, "alpha", &w.teacher, "s1", "published", 3, 2);
    course(&w, "beta", &w.stranger, "s2", "published", 2, 1);
    course(&w, "gamma", &w.teacher, "s1", "published", 1, 1);
    w.conn.execute("UPDATE courses SET title = 'مقدمة في الأمثال', description = 'شرح الإحصاء والجبر' WHERE id = 'alpha'", []).unwrap();
    w.conn.execute("UPDATE courses SET title = 'تحليل 100%' WHERE id = 'beta'", []).unwrap();
    done(&w.conn, &w.me, &a[0], 10);
    let by = |q: LearningQuery| page(&w, &w.me, q);
    // the subject filter
    let p = by(LearningQuery { subject_id: Some("s2".into()), ..Default::default() });
    assert_eq!((ids(&p), p.counts.clone()), (vec!["beta"], LearningCounts { in_progress: 0, not_started: 1, completed: 0 }), "counts follow the subject filter");
    // Arabic folding: no hamza in the query, hamza in the title (title and description are both searched)
    assert_eq!(ids(&by(LearningQuery { q: Some("الامثال".into()), ..Default::default() })), ["alpha"]);
    assert_eq!(ids(&by(LearningQuery { q: Some("احصاء".into()), ..Default::default() })), ["alpha"], "the description is searched, hamza-insensitively");
    assert_eq!(ids(&by(LearningQuery { q: Some("ALPHA".into()), ..Default::default() })).len(), 0, "the title is 'مقدمة...', the id is not searched");
    // wildcards are literal
    assert_eq!(ids(&by(LearningQuery { q: Some("100%".into()), ..Default::default() })), ["beta"]);
    assert!(ids(&by(LearningQuery { q: Some("%".into()), ..Default::default() })) == ["beta"], "a lone % is a percent sign, not 'everything'");
    assert!(by(LearningQuery { q: Some("_".into()), ..Default::default() }).items.is_empty());
    // the counts after a search ignore the chosen tab but not the search
    let p = by(LearningQuery { q: Some("الامثال".into()), state: Some("completed".into()), ..Default::default() });
    assert_eq!((p.items.len(), p.total, p.counts), (0, 0, LearningCounts { in_progress: 1, not_started: 0, completed: 0 }));
    // both filters together
    let p = by(LearningQuery { subject_id: Some("s1".into()), q: Some("الامثال".into()), ..Default::default() });
    assert_eq!(ids(&p), ["alpha"]);
    assert!(by(LearningQuery { subject_id: Some("s2".into()), q: Some("الامثال".into()), ..Default::default() }).items.is_empty());
    // an unknown subject is simply empty
    assert!(by(LearningQuery { subject_id: Some("nope".into()), ..Default::default() }).items.is_empty());
}

#[test]
fn the_page_size_is_24_by_default_and_60_at_most_and_a_bad_state_is_refused() {
    let w = world();
    for n in 0..65 {
        course(&w, &format!("c{n:02}"), &w.teacher, "s1", "published", n, 1);
    }
    let p = page(&w, &w.me, LearningQuery::default());
    assert_eq!((p.items.len(), p.total), (24, 65));
    assert_eq!(page(&w, &w.me, LearningQuery { limit: Some(1000), ..Default::default() }).items.len(), 60);
    assert_eq!(page(&w, &w.me, LearningQuery { limit: Some(0), ..Default::default() }).items.len(), 1, "never an empty page size");
    assert_eq!(page(&w, &w.me, LearningQuery { limit: Some(-5), offset: Some(-9), ..Default::default() }).items.len(), 1);
    assert_eq!(status_of(learning_page(&w.conn, &w.me, &LearningQuery { state: Some("bogus".into()), ..Default::default() })), StatusCode::BAD_REQUEST);
}

#[test]
fn a_description_is_cut_at_160_characters() {
    let w = world();
    course(&w, "c1", &w.teacher, "s1", "published", 1, 1);
    w.conn.execute("UPDATE courses SET description = ?1 WHERE id = 'c1'", params!["ب".repeat(300)]).unwrap();
    let i = learning_items(&w.conn, &w.me.id, None, None).unwrap().remove(0);
    assert_eq!(i.description.as_deref().map(|d| d.chars().count()), Some(160));
    w.conn.execute("UPDATE courses SET description = NULL WHERE id = 'c1'", []).unwrap();
    assert_eq!(learning_items(&w.conn, &w.me.id, None, None).unwrap().remove(0).description, None);
}

#[test]
fn the_overview_cards_come_from_the_same_rows_as_the_learning_list() {
    let w = world();
    let a = course(&w, "a", &w.teacher, "s1", "published", 1, 4);
    let b = course(&w, "b", &w.teacher, "s1", "published", 1, 2);
    course(&w, "c", &w.teacher, "s1", "published", 1, 2); // not started: no card
    let d = course(&w, "d", &w.teacher, "s1", "published", 1, 1); // finished: no card
    done(&w.conn, &w.me, &a[0], 100);
    done(&w.conn, &w.me, &a[2], 150);
    done(&w.conn, &w.me, &b[1], 200);
    done(&w.conn, &w.me, &d[0], 300);
    let overview = crate::platform_student::student_overview(&w.conn, &w.me, NOW).unwrap();
    let listed = page(&w, &w.me, LearningQuery { state: Some("in_progress".into()), ..Default::default() });
    assert_eq!(overview.courses.iter().map(|c| c.course_id.as_str()).collect::<Vec<_>>(), ["b", "a"], "latest activity first, started and unfinished only");
    for card in &overview.courses {
        let item = listed.items.iter().find(|i| i.course_id == card.course_id).unwrap();
        assert_eq!((card.percent, card.next_lesson_id.clone(), card.title.as_str(), card.subject_name.as_str()), (item.percent, item.next_lesson_id.clone(), item.title.as_str(), item.subject_name.as_str()));
    }
    assert_eq!(overview.courses[1].next_lesson_id, Some(a[1].clone()));
}

// ───────── a live session's page ─────────

fn page_of(w: &W, who: &User, id: &str, now: i64) -> Res<StudentLive> {
    student_live(&w.conn, who, id, now)
}

#[test]
fn an_enrolled_student_gets_the_join_link_and_everyone_else_a_locked_page() {
    let w = world();
    live(&w, "l1", &w.teacher, "s1", NOW + 2 * H, 60, "scheduled");
    let mine = page_of(&w, &w.me, "l1", NOW).unwrap();
    assert_eq!((mine.join_url.as_str(), mine.locked, mine.enrolled), ("https://secret.example/join/l1", false, true));
    assert_eq!((mine.title.as_str(), mine.description.as_deref(), mine.teacher_name.as_str(), mine.teacher_id.as_str()), ("l1", Some("وصف"), "المعلم", w.teacher.id.as_str()));
    assert_eq!((mine.subject_id.as_str(), mine.subject_name.as_str(), mine.status.as_str(), mine.starts_at, mine.duration_min), ("s1", "برمجة", "scheduled", NOW + 2 * H, 60));
    assert_eq!((mine.phase, mine.joinable_from), (LivePhase::Upcoming, NOW + 2 * H - 15 * MIN));

    let outsider = page_of(&w, &w.outsider, "l1", NOW).unwrap();
    assert_eq!((outsider.join_url.as_str(), outsider.locked, outsider.enrolled), ("", true, false));
    assert_no_leak(&serde_json::to_string(&outsider).unwrap(), "not enrolled");
    assert_eq!((outsider.title.as_str(), outsider.starts_at, outsider.duration_min), ("l1", NOW + 2 * H, 60), "the rest of the page stays");
    // following the teacher is not enough
    let follower = insert_test_user(&w.conn, "f@x.com", "student", "active");
    follow(&w.conn, &follower, &w.teacher);
    let p = page_of(&w, &follower, "l1", NOW).unwrap();
    assert_eq!((p.join_url.as_str(), p.locked, p.enrolled), ("", true, false));
    assert_no_leak(&serde_json::to_string(&p).unwrap(), "follows the teacher but is not enrolled");
    // enrolling is what unlocks it
    enroll(&w.conn, &w.outsider, "s1");
    assert_eq!(page_of(&w, &w.outsider, "l1", NOW).unwrap().join_url, "https://secret.example/join/l1");
}

#[test]
fn the_phase_changes_at_the_start_and_at_the_end_and_never_in_between() {
    let w = world();
    live(&w, "l1", &w.teacher, "s1", NOW, 30, "scheduled");
    let phase = |now: i64| page_of(&w, &w.me, "l1", now).unwrap().phase;
    assert_eq!(phase(NOW - 1), LivePhase::Upcoming);
    assert_eq!(phase(NOW), LivePhase::LiveNow, "the start itself is live");
    assert_eq!(phase(NOW + 30 * MIN - 1), LivePhase::LiveNow);
    assert_eq!(phase(NOW + 30 * MIN), LivePhase::Ended, "the end itself is over");
    assert_eq!(phase(NOW + 10 * DAY), LivePhase::Ended, "an old session still has a page");
    assert_eq!(page_of(&w, &w.me, "l1", NOW).unwrap().joinable_from, NOW - 15 * MIN);
}

#[test]
fn a_cancelled_or_hidden_session_is_the_same_404_as_one_that_never_existed() {
    let w = world();
    live(&w, "ok", &w.teacher, "s1", NOW + H, 60, "scheduled");
    live(&w, "cancelled", &w.teacher, "s1", NOW + H, 60, "cancelled");
    assert!(page_of(&w, &w.me, "ok", NOW).is_ok());
    assert_eq!(status_of(page_of(&w, &w.me, "cancelled", NOW)), StatusCode::NOT_FOUND);
    assert_eq!(status_of(page_of(&w, &w.me, "missing", NOW)), StatusCode::NOT_FOUND);
    let hidden = |w: &W| match page_of(w, &w.me, "ok", NOW) {
        Ok(_) => false,
        Err(e) => {
            assert_eq!(e.0, StatusCode::NOT_FOUND);
            true
        }
    };
    w.conn.execute("UPDATE teacher_subjects SET status = 'pending' WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
    assert!(hidden(&w), "assignment gone");
    w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
    w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
    assert!(hidden(&w), "subject off");
    w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
    w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
    assert!(hidden(&w), "institution off");
    w.conn.execute("UPDATE institutions SET is_active = 1", []).unwrap();
    assert!(!hidden(&w));
}

// ───────── the schedule ─────────

fn q(from: Option<&str>, to: Option<&str>) -> ScheduleQuery {
    ScheduleQuery { from: from.map(String::from), to: to.map(String::from) }
}

#[test]
fn the_window_defaults_to_two_hours_back_and_a_week_and_refuses_what_makes_no_sense() {
    assert_eq!(schedule_window(&q(None, None), NOW).unwrap(), (NOW - 2 * H, NOW - 2 * H + 7 * DAY));
    assert_eq!(schedule_window(&q(Some("1000"), None), NOW).unwrap(), (1000, 1000 + 7 * DAY), "to = from + 7 days");
    assert_eq!(schedule_window(&q(None, Some(&(NOW + DAY).to_string())), NOW).unwrap(), (NOW - 2 * H, NOW + DAY));
    assert_eq!(schedule_window(&q(Some(" "), Some("")), NOW).unwrap(), (NOW - 2 * H, NOW - 2 * H + 7 * DAY), "blank = absent");
    assert_eq!(schedule_window(&q(Some("0"), Some(&(62 * DAY).to_string())), NOW).unwrap(), (0, 62 * DAY), "62 days exactly is allowed");
    for (from, to, why) in [
        (Some((62 * DAY + 1).to_string()), Some("0".to_string()), "reversed"),
        (Some("0".to_string()), Some((62 * DAY + 1).to_string()), "63 days"),
        (Some("500".to_string()), Some("500".to_string()), "empty window"),
        (Some("abc".to_string()), None, "not a time"),
        (Some("-5".to_string()), Some("10".to_string()), "negative"),
        (None, Some("1".to_string()), "to before the default from"),
    ] {
        assert_eq!(status_of(schedule_window(&ScheduleQuery { from, to }, NOW)), StatusCode::BAD_REQUEST, "{why}");
    }
}

fn sched(w: &W, who: &User, from: i64, to: i64) -> Vec<ScheduleItem> {
    schedule_entries(&w.conn, who, from, to, NOW, SCHEDULE_LIMIT).unwrap().into_iter().map(|e| e.item).collect()
}

fn keys(items: &[ScheduleItem]) -> Vec<(ScheduleKind, &str)> {
    items.iter().map(|i| (i.kind, i.id.as_str())).collect()
}

#[test]
fn live_sessions_are_the_enrolled_subjects_and_the_followed_teachers_and_nobody_elses() {
    let w = world();
    live(&w, "l-mine", &w.teacher, "s1", NOW + H, 60, "scheduled");
    live(&w, "l-followed", &w.stranger, "s2", NOW + 2 * H, 30, "scheduled");
    live(&w, "l-cancelled", &w.teacher, "s1", NOW + 3 * H, 60, "cancelled");
    live(&w, "l-after", &w.teacher, "s1", NOW + 9 * DAY, 60, "scheduled");
    let (from, to) = (NOW - 2 * H, NOW - 2 * H + 7 * DAY);
    // not following the stranger yet
    assert_eq!(keys(&sched(&w, &w.me, from, to)), [(ScheduleKind::Live, "l-mine")]);
    follow(&w.conn, &w.me, &w.stranger);
    let items = sched(&w, &w.me, from, to);
    assert_eq!(keys(&items), [(ScheduleKind::Live, "l-mine"), (ScheduleKind::Live, "l-followed")]);
    let (mine, followed) = (&items[0], &items[1]);
    assert_eq!((mine.locked, mine.link.as_str(), mine.at, mine.ends_at, mine.duration_min), (false, "/platform/live/l-mine", NOW + H, Some(NOW + 2 * H), Some(60)));
    assert_eq!((followed.locked, followed.subject_id.as_str(), followed.subject_name.as_str(), followed.teacher_name.as_str()), (true, "s2", "رياضيات", "الغريب"), "followed but not enrolled: the row is there, the link is not");
    // somebody who neither is enrolled nor follows sees nothing
    assert!(sched(&w, &w.outsider, from, to).is_empty());
    // a hidden session is out whatever the student does
    w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1", params![w.stranger.id]).unwrap();
    assert_eq!(keys(&sched(&w, &w.me, from, to)), [(ScheduleKind::Live, "l-mine")]);
    w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
    assert!(sched(&w, &w.me, from, to).is_empty());
}

#[test]
fn a_session_belongs_to_the_window_when_it_overlaps_it() {
    let w = world();
    live(&w, "ended-before", &w.teacher, "s1", NOW - 5 * H, 60, "scheduled"); // ends NOW - 4h
    live(&w, "running", &w.teacher, "s1", NOW - 3 * H, 240, "scheduled"); // ends NOW + 1h
    live(&w, "ends-at-from", &w.teacher, "s1", NOW - 3 * H, 60, "scheduled"); // ends exactly at from
    live(&w, "starts-at-to", &w.teacher, "s1", NOW + 7 * DAY - 2 * H, 60, "scheduled");
    live(&w, "starts-after-to", &w.teacher, "s1", NOW + 7 * DAY - 2 * H + 1, 60, "scheduled");
    let (from, to) = (NOW - 2 * H, NOW - 2 * H + 7 * DAY);
    let got = sched(&w, &w.me, from, to);
    let got: Vec<&str> = got.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(got, ["ends-at-from", "running", "starts-at-to"], "sorted by start then id: one that ended before the window or starts after it is out, the edges are in");
}

#[test]
fn rows_are_sorted_by_time_then_id_and_cut_at_the_cap() {
    let w = world();
    live(&w, "b", &w.teacher, "s1", NOW + 2 * H, 30, "scheduled");
    live(&w, "a", &w.teacher, "s1", NOW + 2 * H, 30, "scheduled");
    live(&w, "z", &w.teacher, "s1", NOW + H, 30, "scheduled");
    live(&w, "late", &w.teacher, "s1", NOW + 3 * H, 30, "scheduled");
    let all = schedule_entries(&w.conn, &w.me, NOW, NOW + DAY, NOW, SCHEDULE_LIMIT).unwrap();
    assert_eq!(all.iter().map(|e| e.item.id.as_str()).collect::<Vec<_>>(), ["z", "a", "b", "late"]);
    let two = schedule_entries(&w.conn, &w.me, NOW, NOW + DAY, NOW, 2).unwrap();
    assert_eq!(two.iter().map(|e| e.item.id.as_str()).collect::<Vec<_>>(), ["z", "a"], "the earliest rows survive the cut");
}

#[test]
fn exam_openings_and_closings_are_listed_for_the_exams_the_student_can_still_act_on() {
    let w = world();
    exam(&w, "e-open", "s1", Some(NOW - H), Some(NOW + 3 * H)); // open now: closing only
    exam(&w, "e-up", "s1", Some(NOW + 2 * H), Some(NOW + 5 * H)); // opens later: both
    exam(&w, "e-no-window", "s1", None, None);
    exam(&w, "e-past", "s1", Some(NOW - 5 * H), Some(NOW - H)); // already closed by time
    exam(&w, "e-elsewhere", "s2", Some(NOW + H), Some(NOW + 2 * H)); // a subject I am not in
    exam(&w, "e-done", "s1", Some(NOW - H), Some(NOW + 4 * H)); // I have used my only attempt
    exam(&w, "e-closed", "s1", Some(NOW + H), Some(NOW + 6 * H));
    w.conn.execute("UPDATE assessments SET status = 'closed' WHERE id = 'e-closed'", []).unwrap();
    w.conn.execute("INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,opens_at,closes_at,created_at,updated_at) VALUES ('e-draft',?1,?1,'s1','d','[]',1,1,'draft',?2,?3,0,0)", params![w.teacher.id, NOW + H, NOW + 2 * H]).unwrap();
    w.conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status,score,pending,submitted_at) VALUES ('a1','e-done',?1,?2,'submitted',5,0,?2)", params![w.me.id, NOW - 30 * MIN]).unwrap();
    let items = sched(&w, &w.me, NOW - 2 * H, NOW + 7 * DAY);
    assert_eq!(
        keys(&items),
        [(ScheduleKind::ExamOpens, "e-up"), (ScheduleKind::ExamCloses, "e-open"), (ScheduleKind::ExamCloses, "e-up")],
        "opens only while still ahead; closes only while the student can act; no draft, closed, past, finished or foreign exam"
    );
    let opens = &items[0];
    assert_eq!((opens.at, opens.ends_at, opens.duration_min, opens.locked, opens.link.as_str(), opens.subject_name.as_str(), opens.teacher_name.as_str()), (NOW + 2 * H, None, None, false, "/platform/assessments/e-up", "برمجة", "المعلم"));
    assert_eq!(items[1].at, NOW + 3 * H);
    // the window narrows them
    assert!(sched(&w, &w.me, NOW, NOW + H).is_empty());
    assert_eq!(keys(&sched(&w, &w.me, NOW + 2 * H, NOW + 2 * H + 1)), [(ScheduleKind::ExamOpens, "e-up")]);
    // somebody else's exams are not mine
    assert!(sched(&w, &w.outsider, NOW - 2 * H, NOW + 7 * DAY).is_empty());
    // a hidden exam disappears
    w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
    assert!(sched(&w, &w.me, NOW - 2 * H, NOW + 7 * DAY).is_empty());
}

#[test]
fn the_schedule_json_never_has_a_join_link_and_the_entry_has_one_only_when_unlocked() {
    let w = world();
    live(&w, "l-mine", &w.teacher, "s1", NOW + H, 60, "scheduled");
    live(&w, "l-followed", &w.stranger, "s2", NOW + 2 * H, 60, "scheduled");
    follow(&w.conn, &w.me, &w.stranger);
    let entries = schedule_entries(&w.conn, &w.me, NOW, NOW + DAY, NOW, SCHEDULE_LIMIT).unwrap();
    let json = serde_json::to_string(&SchedulePage { items: entries.iter().map(|e| e.item.clone()).collect() }).unwrap();
    assert_no_leak(&json, "the schedule JSON");
    assert!(!json.contains("join_url"), "no join_url key at all");
    assert_eq!(entries[0].join_url.as_deref(), Some("https://secret.example/join/l-mine"));
    assert_eq!(entries[1].join_url, None, "locked: the entry does not even hold the link");
    // the contract's field names
    let v = serde_json::to_value(&entries[0].item).unwrap();
    let mut names: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    names.sort();
    assert_eq!(names, ["at", "duration_min", "ends_at", "id", "kind", "link", "locked", "subject_id", "subject_name", "teacher_name", "title"]);
    assert_eq!(v["kind"], json!("live"));
}

// ───────── the calendar file ─────────

fn entry(kind: ScheduleKind, id: &str, title: &str, at: i64, duration: Option<i64>, join: Option<&str>) -> ScheduleEntry {
    ScheduleEntry {
        join_url: join.map(String::from),
        item: ScheduleItem {
            kind,
            id: id.into(),
            title: title.into(),
            subject_id: "s1".into(),
            subject_name: "برمجة".into(),
            teacher_name: "المعلم".into(),
            at,
            ends_at: duration.map(|d| at + d * MIN),
            duration_min: duration,
            locked: join.is_none() && kind == ScheduleKind::Live,
            link: String::new(),
        },
    }
}

#[test]
fn utc_stamps_are_exact_across_leap_days_and_year_ends() {
    assert_eq!(ics_utc(0), "19700101T000000Z");
    assert_eq!(ics_utc(NOW), "20270115T080000Z");
    assert_eq!(ics_utc(951_782_400_000), "20000229T000000Z", "a leap day");
    assert_eq!(ics_utc(951_782_400_000 + 86_400_000), "20000301T000000Z");
    assert_eq!(ics_utc(4_102_444_799_000), "20991231T235959Z");
    assert_eq!(ics_utc(-1_000), "19691231T235959Z", "before the epoch does not panic");
    assert_eq!(ics_utc(1_700_000_000_999), "20231114T221320Z", "milliseconds are dropped, not rounded");
}

#[test]
fn text_is_escaped_and_control_characters_cannot_inject_a_line() {
    assert_eq!(ics_escape(r"a\b;c,d"), r"a\\b\;c\,d");
    assert_eq!(ics_escape("one\ntwo"), r"one\ntwo");
    assert_eq!(ics_escape("x\r\nEND:VEVENT\r\nBEGIN:VEVENT"), r"x\nEND:VEVENT\nBEGIN:VEVENT", "a CR is dropped and the LF becomes a visible escape: one logical line");
    assert_eq!(ics_escape("tab\there\u{0}nul\u{7}bell"), "tabherenulbell");
    assert_eq!(ics_escape("عربي، نص"), "عربي، نص", "the Arabic comma is not an ASCII comma");
}

fn unfold(s: &str) -> String {
    s.replace("\r\n ", "")
}

#[test]
fn long_lines_fold_at_75_octets_without_splitting_a_character() {
    for line in ["SUMMARY:".to_string() + &"x".repeat(200), "SUMMARY:".to_string() + &"عنوان طويل جدا ".repeat(20), "DESCRIPTION:".to_string() + &"🙂".repeat(60)] {
        let mut out = String::new();
        ics_fold(&line, &mut out);
        assert!(out.ends_with("\r\n"));
        for physical in out.trim_end_matches("\r\n").split("\r\n") {
            assert!(physical.len() <= 75, "{} octets: {physical}", physical.len());
        }
        assert!(out.matches("\r\n").count() > 1, "it really folded");
        assert!(out.split("\r\n").skip(1).filter(|l| !l.is_empty()).all(|l| l.starts_with(' ')), "every continuation starts with a space");
        assert_eq!(unfold(&out).trim_end_matches("\r\n"), line, "unfolding gives the line back");
    }
    // a short line is untouched
    let mut out = String::new();
    ics_fold("BEGIN:VCALENDAR", &mut out);
    assert_eq!(out, "BEGIN:VCALENDAR\r\n");
    // exactly 75 octets stays on one line, 76 folds
    let mut out = String::new();
    ics_fold(&"y".repeat(75), &mut out);
    assert_eq!(out.matches("\r\n").count(), 1);
    let mut out = String::new();
    ics_fold(&"y".repeat(76), &mut out);
    assert_eq!(out.matches("\r\n").count(), 2);
}

#[test]
fn the_calendar_file_has_the_agreed_events_and_nothing_private() {
    let entries = vec![
        entry(ScheduleKind::Live, "l1", "درس, الأول; مهم", NOW + H, Some(90), Some("https://secret.example/join/l1")),
        entry(ScheduleKind::Live, "l2", "locked", NOW + 2 * H, Some(30), None),
        entry(ScheduleKind::ExamOpens, "e1", "Midterm", NOW + 3 * H, None, None),
        entry(ScheduleKind::ExamCloses, "e1", "Midterm", NOW + 5 * H, None, None),
    ];
    let ics = build_ics(&entries, NOW, IcsLang::En);
    let expected = [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//azlearn//schedule//AR",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
        "X-WR-CALNAME:azlearn",
        // a live session: UTC start and end, escaped text, the join link only here, a 15-minute alarm
        "BEGIN:VEVENT",
        "UID:live-l1@azlearn",
        "DTSTAMP:20270115T080000Z",
        "DTSTART:20270115T090000Z",
        "DTEND:20270115T103000Z",
        "SUMMARY:درس\\, الأول\\; مهم",
        "DESCRIPTION:Teacher: المعلم\\nSubject: برمجة\\nJoin link: https://secret.example/join/l1",
        "BEGIN:VALARM",
        "ACTION:DISPLAY",
        "DESCRIPTION:درس\\, الأول\\; مهم",
        "TRIGGER:-PT15M",
        "END:VALARM",
        "END:VEVENT",
        // locked: same event, no link
        "BEGIN:VEVENT",
        "UID:live-l2@azlearn",
        "DTSTAMP:20270115T080000Z",
        "DTSTART:20270115T100000Z",
        "DTEND:20270115T103000Z",
        "SUMMARY:locked",
        "DESCRIPTION:Teacher: المعلم\\nSubject: برمجة",
        "BEGIN:VALARM",
        "ACTION:DISPLAY",
        "DESCRIPTION:locked",
        "TRIGGER:-PT15M",
        "END:VALARM",
        "END:VEVENT",
        // exams: 15-minute blocks starting at the opening time / ending at the closing time, no alarm
        "BEGIN:VEVENT",
        "UID:exam-e1-opens@azlearn",
        "DTSTAMP:20270115T080000Z",
        "DTSTART:20270115T110000Z",
        "DTEND:20270115T111500Z",
        "SUMMARY:Exam opens: Midterm",
        "DESCRIPTION:Teacher: المعلم\\nSubject: برمجة",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:exam-e1-closes@azlearn",
        "DTSTAMP:20270115T080000Z",
        "DTSTART:20270115T124500Z",
        "DTEND:20270115T130000Z",
        "SUMMARY:Exam closes: Midterm",
        "DESCRIPTION:Teacher: المعلم\\nSubject: برمجة",
        "END:VEVENT",
        "END:VCALENDAR",
    ]
    .join("\r\n")
        + "\r\n";
    assert_eq!(unfold(&ics), expected);
    // CRLF everywhere: no bare LF, no bare CR
    assert!(!ics.replace("\r\n", "").contains(['\r', '\n']));
    assert!(ics.split("\r\n").all(|l| l.len() <= 75), "no physical line over 75 octets");
    // the locked event never carries the link, nor does the file name any e-mail address
    let locked_event = ics.split("BEGIN:VEVENT").nth(2).unwrap();
    assert_no_leak(locked_event, "the locked event");
    assert_eq!(ics.matches('@').count(), ics.matches("@azlearn").count(), "only the UID domains contain an @: no e-mail address");
    // Arabic wording by default
    let ar = build_ics(&entries[2..3], NOW, IcsLang::Ar);
    assert!(unfold(&ar).contains("SUMMARY:فتح الامتحان: Midterm"));
}

#[test]
fn a_title_with_line_breaks_stays_one_property() {
    let e = entry(ScheduleKind::Live, "l\r\nEND:VCALENDAR", "bad\r\nBEGIN:VEVENT\nSUMMARY:evil", NOW + H, Some(60), None);
    let ics = build_ics(&[e], NOW, IcsLang::En);
    let lines: Vec<String> = unfold(&ics).split("\r\n").map(String::from).collect();
    let count = |l: &str| lines.iter().filter(|x| x.as_str() == l).count();
    assert_eq!((count("BEGIN:VEVENT"), count("END:VEVENT"), count("END:VCALENDAR")), (1, 1, 1), "the injected lines are text, not structure");
    assert!(lines.contains(&"SUMMARY:bad\\nBEGIN:VEVENT\\nSUMMARY:evil".to_string()), "{lines:?}");
    assert!(lines.contains(&"UID:live-lENDVCALENDAR@azlearn".to_string()), "the unsafe characters of the id were dropped: {lines:?}");
}

#[test]
fn an_empty_schedule_is_still_a_valid_calendar() {
    let ics = build_ics(&[], NOW, IcsLang::Ar);
    assert_eq!(ics, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//azlearn//schedule//AR\r\nCALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\nX-WR-CALNAME:azlearn\r\nEND:VCALENDAR\r\n");
}

#[test]
fn the_file_covers_yesterday_to_ninety_days_ahead() {
    let w = world();
    live(&w, "yesterday-ok", &w.teacher, "s1", NOW - DAY + H, 30, "scheduled");
    live(&w, "too-old", &w.teacher, "s1", NOW - 3 * DAY, 30, "scheduled");
    live(&w, "day-89", &w.teacher, "s1", NOW + 89 * DAY, 30, "scheduled");
    live(&w, "day-91", &w.teacher, "s1", NOW + 91 * DAY, 30, "scheduled");
    let got = schedule_entries(&w.conn, &w.me, NOW - ICS_BEFORE_MS, NOW + ICS_AFTER_MS, NOW, ICS_LIMIT).unwrap();
    assert_eq!(got.iter().map(|e| e.item.id.as_str()).collect::<Vec<_>>(), ["yesterday-ok", "day-89"]);
}

/// A tripwire for the next person who adds an endpoint: the join links of live sessions and the video links of lessons are
/// only ever read by the two modules that apply `platform_content::can_see_links` (`platform_content` itself, and this one for
/// the student's live page, schedule and calendar file). Another module that needs them must go through those, not around
/// them — and this test is the place to decide that on purpose.
#[test]
fn only_the_two_modules_that_lock_links_read_them() {
    const ALLOWED: [&str; 2] = ["platform_content.rs", "platform_student_learning.rs"];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map_or(true, |e| e != "rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).unwrap();
        let code = src.split("#[cfg(test)]").next().unwrap(); // tests insert such rows; production code must not read them
        checked += 1;
        if ["join_url", "video_url", "embed_url"].iter().any(|t| code.contains(t)) {
            assert!(ALLOWED.contains(&name.as_str()), "{name} reads a join / lesson video link: it must go through platform_content::can_see_links (and be added to this list on purpose)");
        }
    }
    assert!(checked > 20, "the scan really looked at the modules ({checked})");
}
