//! The student's learning routes — and every other route that serialises a live session or a lesson — through the real
//! handlers, so the role wall and the **link locking** are checked where they are enforced: a join link or a lesson video
//! link that a viewer may not see must not appear anywhere in ANY response to that viewer (the assertions look at the
//! response text, not at one field).

use super::tests::{assert_no_leak, course_in, done, enroll, follow, live_in, world, H, MIN, W};
use super::*;
use crate::platform::{insert_test_user, new_id, PlatformState};
use axum::body::{to_bytes, Body};
use axum::http::Request;
use axum::http::Method;
use serde_json::{json, Value};
use std::collections::HashMap;
use tower::ServiceExt;

struct Users {
    /// enrolled in s1
    me: User,
    /// enrolled nowhere, follows nobody
    outsider: User,
    teacher: User,
    /// approved for s1 as well
    colleague: User,
}

struct Harness {
    app: Router,
    tokens: HashMap<&'static str, String>,
    state: Arc<AppState>,
    u: Users,
}

impl Harness {
    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.state.platform.conn.lock().unwrap()
    }
}

fn token_for(conn: &Connection, u: &User) -> String {
    let token = format!("tok-{}", new_id());
    conn.execute("INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, 0, ?3)", params![crate::relay::sha256_hex(&token), u.id, now_ms() + 3_600_000]).unwrap();
    token
}

fn app(state: Arc<AppState>) -> Router {
    use axum::routing::get;
    Router::new()
        .merge(super::routes())
        .merge(crate::platform_bookmarks::routes())
        .merge(crate::platform_student::routes())
        .route("/api/platform/courses/{id}", get(crate::platform_content::get_course_handler))
        .route("/api/platform/subjects/{id}/content", get(crate::platform_content::subject_content_handler))
        .route("/api/platform/teachers/{id}/content", get(crate::platform_content::teacher_content_handler))
        .route("/api/platform/feed", get(crate::platform_content::feed_handler))
        .route("/api/platform/live/{id}", get(crate::platform_content::get_live_handler))
        .route("/api/platform/my/progress", get(crate::platform_engage::my_progress_handler))
        .merge(crate::platform_search::routes())
        .route("/api/platform/notifications", get(crate::platform_engage::notifications_handler))
        .route("/api/platform/subjects/{id}", get(crate::platform_learning::subject_page_handler))
        .route("/api/platform/teachers/{id}", get(crate::platform_learning::teacher_page_handler))
        // the directory is a static segment next to `{id}`, exactly as main.rs registers them
        .merge(crate::platform_learning::directory_routes())
        .with_state(state)
}

/// The world of `tests.rs` plus a student who only follows the teacher, a suspended student, a pending teacher, a published
/// course `c1` ("دورة بايثون") with three lessons (YouTube, direct file, plain link), a live session `l1` ("بث مباشر") in
/// two hours, and a session token per person.
fn harness() -> Harness {
    static CFG: std::sync::Once = std::sync::Once::new();
    CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-learning-http-test-{}", new_id()))));
    let w = world();
    let follower = insert_test_user(&w.conn, "f@x.com", "student", "active");
    follow(&w.conn, &follower, &w.teacher);
    let suspended = insert_test_user(&w.conn, "sus@x.com", "student", "suspended");
    let pending = insert_test_user(&w.conn, "pend@x.com", "teacher", "pending");
    course_in(&w.conn, "c1", &w.teacher, "s1", "published", 5, 3);
    w.conn.execute("UPDATE courses SET title = 'دورة بايثون', description = 'شرح للمبتدئين' WHERE id = 'c1'", []).unwrap();
    live_in(&w.conn, "l1", &w.teacher, "s1", now_ms() + 2 * H, 60, "scheduled");
    w.conn.execute("UPDATE live_sessions SET title = 'بث مباشر' WHERE id = 'l1'", []).unwrap();
    let mut tokens = HashMap::new();
    for (key, u) in [
        ("me", &w.me),
        ("outsider", &w.outsider),
        ("follower", &follower),
        ("teacher", &w.teacher),
        ("colleague", &w.colleague),
        ("stranger", &w.stranger),
        ("admin", &w.admin),
        ("suspended", &suspended),
        ("pending", &pending),
    ] {
        tokens.insert(key, token_for(&w.conn, u));
    }
    let W { conn, teacher, colleague, me, outsider, .. } = w;
    let state = Arc::new(AppState {
        config_store: exameow_core::config::ConfigStore::new("ExameowServerLearningHttpTest").expect("a scratch config dir"),
        relay: crate::relay::init_db(":memory:").unwrap(),
        platform: PlatformState::for_tests(conn),
        admin_token: std::sync::Mutex::new("pass".into()),
        legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
        auth_failures: crate::token_limit::TokenLimiter::new(600),
    });
    Harness { app: app(state.clone()), tokens, state, u: Users { me, outsider, teacher, colleague } }
}

async fn send(h: &Harness, method: Method, uri: &str, who: Option<&str>, body: Option<Value>) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(w) = who {
        b = b.header(header::AUTHORIZATION, format!("Bearer {}", h.tokens[w]));
    }
    let res = h.app.clone().oneshot(b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string()))).unwrap()).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn get_text(h: &Harness, uri: &str, who: &str) -> (StatusCode, String) {
    let (st, _, body) = send(h, Method::GET, uri, Some(who), None).await;
    (st, body)
}

async fn get_json(h: &Harness, uri: &str, who: &str) -> (StatusCode, Value) {
    let (st, body) = get_text(h, uri, who).await;
    (st, serde_json::from_str(&body).unwrap_or(Value::Null))
}

const SECRET_JOIN: &str = "https://secret.example/join/l1";

/// (token key, may see links, is a student)
const VIEWERS: [(&str, bool, bool); 7] = [
    ("outsider", false, true),
    ("follower", false, true),
    ("me", true, true),
    ("teacher", true, false),
    ("colleague", true, false),
    ("stranger", false, false),
    ("admin", true, false),
];

// ───────── the matrix ─────────

#[tokio::test]
async fn no_withheld_link_reaches_a_viewer_who_may_not_see_it_in_any_endpoint() {
    let h = harness();
    {
        let db = h.db();
        done(&db, &h.u.me, "c1-l1", now_ms() - MIN);
        db.execute("INSERT INTO bookmarks(user_id,target_type,target_id,created_at) VALUES (?1,'course','c1',1)", params![h.u.me.id]).unwrap();
    }
    let teacher_content = format!("/api/platform/teachers/{}/content", h.u.teacher.id);
    for (who, unlocked, student) in VIEWERS {
        // ── the course
        let (st, body) = get_text(&h, "/api/platform/courses/c1", who).await;
        assert_eq!(st, StatusCode::OK, "{who}: {body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let lessons = v["lessons"].as_array().unwrap();
        assert_eq!(lessons.len(), 3, "{who}: the lessons are listed whoever the viewer is (content is open to browse)");
        assert_eq!(v["title"], json!("دورة بايثون"), "{who}");
        assert_eq!(v["enrolled"], json!(who == "me"), "{who}: `enrolled` is the student's own enrolment");
        assert_eq!(v["links_locked"], json!(!unlocked), "{who}");
        let kinds: Vec<&str> = lessons.iter().map(|l| l["video_kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["youtube", "file", "link"], "{who}: the kind stays, so the page can say what it is");
        for l in lessons {
            assert_eq!(l["locked"], json!(!unlocked), "{who}");
            assert!(l["title"].as_str().unwrap().starts_with("c1-l"), "{who}: titles stay");
            if unlocked {
                assert!(l["video_url"].as_str().unwrap().starts_with("https://"), "{who}: {l}");
            } else {
                assert_eq!((l["video_url"].clone(), l["embed_url"].clone()), (json!(""), Value::Null), "{who}: {l}");
            }
        }
        if unlocked {
            assert_eq!(lessons[0]["embed_url"], json!("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"), "{who}");
        } else {
            assert_no_leak(&body, &format!("{who}: GET /courses/c1"));
        }

        // ── the bundles
        for uri in ["/api/platform/subjects/s1/content", teacher_content.as_str()] {
            let (st, body) = get_text(&h, uri, who).await;
            assert_eq!(st, StatusCode::OK, "{who} {uri}");
            let v: Value = serde_json::from_str(&body).unwrap();
            let sessions = v["live"].as_array().unwrap();
            assert_eq!(sessions.len(), 1, "{who} {uri}: the session is listed for everybody");
            assert_eq!((sessions[0]["locked"].clone(), sessions[0]["join_url"].clone()), (json!(!unlocked), json!(if unlocked { SECRET_JOIN } else { "" })), "{who} {uri}");
            assert_eq!(v["courses"].as_array().unwrap().len(), 1, "{who} {uri}");
            if !unlocked {
                assert_no_leak(&body, &format!("{who}: GET {uri}"));
            }
        }

        // ── the live page
        let (st, body) = get_text(&h, "/api/platform/live/l1", who).await;
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if student {
            assert_eq!(st, StatusCode::OK, "{who}: {body}");
            assert_eq!((v["locked"].clone(), v["join_url"].clone(), v["enrolled"].clone()), (json!(!unlocked), json!(if unlocked { SECRET_JOIN } else { "" }), json!(who == "me")), "{who}");
            assert_eq!((v["phase"].clone(), v["status"].clone()), (json!("upcoming"), json!("scheduled")), "{who}");
        } else if who == "teacher" || who == "admin" {
            assert_eq!(st, StatusCode::OK, "{who}: {body}");
            assert_eq!(v["join_url"], json!(SECRET_JOIN), "{who}: the owner and an admin edit the session");
            assert!(v.get("phase").is_none() && v.get("ended").is_some(), "{who}: the owner shape is unchanged");
        } else {
            assert_eq!(st, StatusCode::FORBIDDEN, "{who}: another teacher gets 403 as before, even an approved colleague");
        }
        if !unlocked {
            assert_no_leak(&body, &format!("{who}: GET /live/l1"));
        }

        // ── the student-only endpoints that carry sessions
        if student {
            let (st, body) = get_text(&h, "/api/platform/feed", who).await;
            assert_eq!(st, StatusCode::OK, "{who}");
            let v: Value = serde_json::from_str(&body).unwrap();
            let sessions = v["live"].as_array().unwrap();
            if who == "outsider" {
                assert!(sessions.is_empty(), "outsider: neither enrolled nor following");
            } else {
                assert_eq!(sessions.len(), 1, "{who}");
                assert_eq!((sessions[0]["locked"].clone(), sessions[0]["join_url"].clone()), (json!(!unlocked), json!(if unlocked { SECRET_JOIN } else { "" })), "{who}: feed");
            }
            if !unlocked {
                assert_no_leak(&body, &format!("{who}: GET /feed"));
            }

            let (st, body) = get_text(&h, "/api/platform/my/schedule", who).await;
            assert_eq!(st, StatusCode::OK, "{who}");
            assert_no_leak(&body, &format!("{who}: GET /my/schedule (never carries a join link, even for the enrolled)"));
            assert!(!body.contains("join_url"), "{who}: no join_url key in the schedule");
            let v: Value = serde_json::from_str(&body).unwrap();
            let items = v["items"].as_array().unwrap();
            if who == "outsider" {
                assert!(items.is_empty());
            } else {
                assert_eq!((items.len(), items[0]["id"].clone(), items[0]["kind"].clone(), items[0]["locked"].clone()), (1, json!("l1"), json!("live"), json!(!unlocked)), "{who}");
            }

            let (st, headers, ics) = send(&h, Method::GET, "/api/platform/my/schedule.ics", Some(who), None).await;
            assert_eq!(st, StatusCode::OK, "{who}");
            assert_eq!(headers[header::CONTENT_TYPE], "text/calendar; charset=utf-8");
            assert_eq!(ics.contains("UID:live-l1@azlearn"), who != "outsider", "{who}");
            assert_eq!(ics.contains("secret.example"), unlocked, "{who}: the calendar file carries the join link only for a student who may open it");
            if !unlocked {
                assert_no_leak(&ics, &format!("{who}: GET /my/schedule.ics"));
            }
        }

        // ── endpoints that never carry a link: nothing of it may appear, for anybody
        let mut never = vec!["/api/platform/search?q=%D8%A8%D8%A7%D9%8A%D8%AB%D9%88%D9%86".to_string(), "/api/platform/search?q=%D9%85%D8%A8%D8%A7%D8%B4%D8%B1".to_string(), "/api/platform/notifications".into()];
        never.extend(["/api/platform/subjects/s1".to_string(), format!("/api/platform/teachers/{}", h.u.teacher.id), "/api/platform/teachers/directory".into()]);
        if student {
            never.extend(["/api/platform/student/overview", "/api/platform/my/learning", "/api/platform/my/progress", "/api/platform/my/bookmarks", "/api/platform/my/following"].map(String::from));
        }
        for uri in never {
            let (st, body) = get_text(&h, &uri, who).await;
            assert_eq!(st, StatusCode::OK, "{who} {uri}: {body}");
            assert_no_leak(&body, &format!("{who}: GET {uri}"));
        }
    }
}

#[tokio::test]
async fn the_endpoints_of_the_matrix_really_carry_data_for_the_enrolled_student() {
    // guards the matrix against passing because every response was empty
    let h = harness();
    done(&h.db(), &h.u.me, "c1-l1", now_ms() - MIN);
    let (_, v) = get_json(&h, "/api/platform/my/learning", "me").await;
    assert_eq!((v["total"].clone(), v["items"][0]["course_id"].clone(), v["items"][0]["state"].clone()), (json!(1), json!("c1"), json!("in_progress")));
    let (_, v) = get_json(&h, "/api/platform/my/progress", "me").await;
    assert_eq!((v[0]["course_id"].clone(), v[0]["completed"].clone(), v[0]["next_lesson_id"].clone(), v[0]["subject_id"].clone()), (json!("c1"), json!(1), json!("c1-l2"), json!("s1")));
    assert_eq!(v[0]["percent"], json!(33.33));
    let (_, v) = get_json(&h, "/api/platform/student/overview", "me").await;
    assert_eq!((v["courses"][0]["course_id"].clone(), v["courses"][0]["next_lesson_id"].clone(), v["next_live"]["id"].clone()), (json!("c1"), json!("c1-l2"), json!("l1")));
    let (_, v) = get_json(&h, "/api/platform/search?q=%D8%A8%D8%A7%D9%8A%D8%AB%D9%88%D9%86", "me").await;
    assert_eq!(v["courses"][0]["id"], json!("c1"));
    let (_, v) = get_json(&h, "/api/platform/search?q=%D9%85%D8%A8%D8%A7%D8%B4%D8%B1", "me").await;
    assert_eq!((v["live"][0]["id"].clone(), v["counts"]["live"].clone()), (json!("l1"), json!(1)));
}

// ───────── shapes, walls and validation ─────────

#[tokio::test]
async fn the_live_page_answers_each_role_in_its_own_shape() {
    let h = harness();
    let (_, v) = get_json(&h, "/api/platform/live/l1", "me").await;
    let mut names: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    names.sort();
    assert_eq!(
        names,
        ["description", "duration_min", "enrolled", "id", "join_url", "joinable_from", "locked", "phase", "starts_at", "status", "subject_id", "subject_name", "teacher_id", "teacher_name", "title"],
        "the student shape is exactly the agreed contract"
    );
    assert_eq!((v["teacher_name"].clone(), v["subject_name"].clone()), (json!("المعلم"), json!("برمجة")));
    let starts = v["starts_at"].as_i64().unwrap();
    assert_eq!(v["joinable_from"], json!(starts - 15 * MIN));
    // the owner's shape is the editing one
    let (_, v) = get_json(&h, "/api/platform/live/l1", "teacher").await;
    assert!(v.get("ended").is_some() && v.get("visible").is_some() && v.get("phase").is_none(), "{v}");
    // a cancelled session is a 404 for a student and still there for its owner
    h.db().execute("UPDATE live_sessions SET status = 'cancelled' WHERE id = 'l1'", []).unwrap();
    let (st, v) = get_json(&h, "/api/platform/live/l1", "me").await;
    assert_eq!((st, v["error"].clone()), (StatusCode::NOT_FOUND, json!("not_found")));
    assert_eq!(get_json(&h, "/api/platform/live/l1", "teacher").await.0, StatusCode::OK);
    assert_eq!(get_json(&h, "/api/platform/live/nope", "me").await.0, StatusCode::NOT_FOUND);
    // not signed in / not an active account
    assert_eq!(send(&h, Method::GET, "/api/platform/live/l1", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get_json(&h, "/api/platform/live/l1", "suspended").await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn only_an_active_student_reaches_the_student_only_routes() {
    let h = harness();
    let routes = [
        (Method::GET, "/api/platform/my/learning", None),
        (Method::GET, "/api/platform/my/schedule", None),
        (Method::GET, "/api/platform/my/schedule.ics", None),
        (Method::GET, "/api/platform/my/bookmarks", None),
        (Method::GET, "/api/platform/my/following", None),
        (Method::PUT, "/api/platform/bookmarks", Some(json!({"target_type": "course", "target_id": "c1"}))),
        (Method::DELETE, "/api/platform/bookmarks/course/c1", None),
    ];
    for (m, uri, body) in routes {
        for who in ["teacher", "admin", "suspended", "pending"] {
            let (st, _, text) = send(&h, m.clone(), uri, Some(who), body.clone()).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{who} {m} {uri}: {text}");
        }
        assert_eq!(send(&h, m.clone(), uri, None, body.clone()).await.0, StatusCode::UNAUTHORIZED, "no session: {uri}");
        let (st, _, text) = send(&h, m.clone(), uri, Some("me"), body.clone()).await;
        assert!(st.is_success(), "student {m} {uri}: {st} {text}");
    }
    // the open routes: any active account (the page of the search and of the directory)
    for who in ["teacher", "admin", "me"] {
        assert_eq!(get_json(&h, "/api/platform/teachers/directory", who).await.0, StatusCode::OK, "{who}");
        assert_eq!(get_json(&h, "/api/platform/search?q=%D8%A8%D8%A7%D9%8A%D8%AB%D9%88%D9%86", who).await.0, StatusCode::OK, "{who}");
    }
    assert_eq!(get_json(&h, "/api/platform/teachers/directory", "suspended").await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_new_routes_answer_in_the_agreed_shapes() {
    let h = harness();
    let (_, v) = get_json(&h, "/api/platform/my/learning", "me").await;
    assert_eq!(v, json!({"items": [serde_json::to_value(learning_items(&h.db(), &h.u.me.id, None, None).unwrap().remove(0)).unwrap()], "total": 1, "counts": {"in_progress": 0, "not_started": 1, "completed": 0}}));
    let mut names: Vec<String> = v["items"][0].as_object().unwrap().keys().cloned().collect();
    names.sort();
    assert_eq!(names, ["completed", "course_id", "description", "last_at", "next_lesson_id", "percent", "state", "subject_id", "subject_name", "teacher_id", "teacher_name", "title", "total", "updated_at"]);
    let (st, v) = get_json(&h, "/api/platform/my/learning?state=bogus", "me").await;
    assert_eq!((st, v["error"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_filter")));
    let (_, v) = get_json(&h, "/api/platform/my/learning?state=completed&q=%D8%A8%D8%A7%D9%8A%D8%AB%D9%88%D9%86&subject_id=s1&limit=5&offset=0", "me").await;
    assert_eq!((v["items"].clone(), v["total"].clone(), v["counts"]["not_started"].clone()), (json!([]), json!(0), json!(1)));
    let (_, v) = get_json(&h, "/api/platform/my/following", "follower").await;
    assert_eq!((v["items"][0]["id"].clone(), v["items"][0]["subject_count"].clone()), (json!(h.u.teacher.id), json!(1)));
    assert_eq!(get_json(&h, "/api/platform/my/following", "me").await.1, json!({"items": []}));
    let (_, v) = get_json(&h, "/api/platform/my/bookmarks", "me").await;
    assert_eq!(v, json!({"items": []}));
}

#[tokio::test]
async fn the_schedule_route_validates_its_window_and_defaults_to_the_coming_week() {
    let h = harness();
    // default: two hours back, a week ahead: l1 (in two hours) is in
    let (st, v) = get_json(&h, "/api/platform/my/schedule", "me").await;
    assert_eq!((st, v["items"][0]["id"].clone()), (StatusCode::OK, json!("l1")));
    let now = now_ms();
    for (qs, ok) in [
        (format!("from={}&to={}", now, now + 3 * H), true),
        (format!("from={}", now), true),
        (format!("to={}", now + H), true),
        (format!("from={}&to={}", now, now), false),
        (format!("from={}&to={}", now + H, now), false),
        (format!("from={}&to={}", now, now + 63 * 86_400_000), false),
        ("from=abc".to_string(), false),
        ("to=-1".to_string(), false),
    ] {
        let (st, v) = get_json(&h, &format!("/api/platform/my/schedule?{qs}"), "me").await;
        if ok {
            assert_eq!(st, StatusCode::OK, "{qs}: {v}");
        } else {
            assert_eq!((st, v["error"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_range")), "{qs}");
        }
    }
    // a window before the session: empty; one around it: there
    let (_, v) = get_json(&h, &format!("/api/platform/my/schedule?from={}&to={}", now, now + H), "me").await;
    assert_eq!(v["items"], json!([]));
    let (_, v) = get_json(&h, &format!("/api/platform/my/schedule?from={}&to={}", now + H, now + 3 * H), "me").await;
    assert_eq!(v["items"][0]["link"], json!("/platform/live/l1"));
    // another student's schedule is never mine: the follower sees l1 as locked, the outsider nothing
    let (_, v) = get_json(&h, "/api/platform/my/schedule", "follower").await;
    assert_eq!((v["items"][0]["locked"].clone(), v["items"].as_array().unwrap().len()), (json!(true), 1));
    assert_eq!(get_json(&h, "/api/platform/my/schedule", "outsider").await.1["items"], json!([]));
}

#[tokio::test]
async fn the_calendar_download_has_the_agreed_headers_and_is_rate_limited() {
    let h = harness();
    let (st, headers, body) = send(&h, Method::GET, "/api/platform/my/schedule.ics", Some("me"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/calendar; charset=utf-8");
    assert_eq!(headers[header::CONTENT_DISPOSITION], "attachment; filename=\"azlearn-schedule.ics\"");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(body.starts_with("BEGIN:VCALENDAR\r\n") && body.ends_with("END:VCALENDAR\r\n"));
    assert!(body.contains("PRODID:-//azlearn//schedule//AR\r\n") && body.contains("X-WR-CALNAME:azlearn\r\n"));
    assert!(body.contains("TRIGGER:-PT15M\r\n"), "a live event carries its alarm");
    assert!(!body.replace("\r\n", "").contains('\n'), "CRLF everywhere");
    // 10 downloads a minute per student: the first ten succeed (this one was the first), a burst is refused
    let mut statuses = vec![st];
    for _ in 0..24 {
        statuses.push(send(&h, Method::GET, "/api/platform/my/schedule.ics", Some("me"), None).await.0);
    }
    assert!(statuses[..10].iter().all(|s| *s == StatusCode::OK), "{statuses:?}");
    assert!(statuses.contains(&StatusCode::TOO_MANY_REQUESTS), "{statuses:?}");
    // another student has their own budget
    assert_eq!(send(&h, Method::GET, "/api/platform/my/schedule.ics", Some("follower"), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_teachers_directory_route_is_not_taken_for_a_teacher_id() {
    let h = harness();
    let (st, v) = get_json(&h, "/api/platform/teachers/directory", "me").await;
    assert_eq!(st, StatusCode::OK);
    assert!(v["items"].is_array() && v["total"].is_number(), "{v}");
    let (st, v) = get_json(&h, &format!("/api/platform/teachers/{}", h.u.teacher.id), "me").await;
    assert_eq!((st, v["id"].clone()), (StatusCode::OK, json!(h.u.teacher.id)));
}

#[tokio::test]
async fn enrolling_unlocks_everything_at_once() {
    let h = harness();
    let (_, v) = get_json(&h, "/api/platform/courses/c1", "outsider").await;
    assert_eq!(v["links_locked"], json!(true));
    enroll(&h.db(), &h.u.outsider, "s1");
    let (_, v) = get_json(&h, "/api/platform/courses/c1", "outsider").await;
    assert_eq!((v["links_locked"].clone(), v["enrolled"].clone(), v["lessons"][1]["video_url"].clone()), (json!(false), json!(true), json!("https://secret.example/c1/video.mp4")));
    let (_, v) = get_json(&h, "/api/platform/live/l1", "outsider").await;
    assert_eq!((v["locked"].clone(), v["join_url"].clone()), (json!(false), json!(SECRET_JOIN)));
    // and withdrawing the enrolment locks it again
    h.db().execute("DELETE FROM subject_enrollments WHERE student_id = ?1", params![h.u.outsider.id]).unwrap();
    let (_, body) = get_text(&h, "/api/platform/courses/c1", "outsider").await;
    assert_no_leak(&body, "after unenrolling");
}

#[tokio::test]
async fn a_teacher_whose_approval_was_withdrawn_loses_the_links_of_a_class_that_is_not_theirs() {
    let h = harness();
    assert_eq!(get_json(&h, "/api/platform/courses/c1", "colleague").await.1["links_locked"], json!(false));
    h.db().execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's1'", params![h.u.colleague.id]).unwrap();
    let (_, body) = get_text(&h, "/api/platform/courses/c1", "colleague").await;
    assert_no_leak(&body, "a colleague whose approval is gone");
    // the owner keeps their own content even if they are no longer approved (they can still edit it)
    h.db().execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's1'", params![h.u.teacher.id]).unwrap();
    let (st, v) = get_json(&h, "/api/platform/courses/c1", "teacher").await;
    assert_eq!((st, v["links_locked"].clone()), (StatusCode::OK, json!(false)));
    // while students no longer see the hidden course at all
    assert_eq!(get_json(&h, "/api/platform/courses/c1", "me").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bookmarks_over_http_answer_with_the_agreed_statuses() {
    let h = harness();
    let put = |who: &'static str, t: &str, id: &str| {
        let body = json!({ "target_type": t, "target_id": id });
        let h = &h;
        let who = who;
        async move { send(h, Method::PUT, "/api/platform/bookmarks", Some(who), Some(body)).await }
    };
    let (st, _, body) = put("me", "course", "c1").await;
    assert_eq!((st, body.as_str()), (StatusCode::NO_CONTENT, ""));
    assert_eq!(put("me", "course", "c1").await.0, StatusCode::NO_CONTENT, "idempotent");
    let (_, v) = get_json(&h, "/api/platform/my/bookmarks", "me").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let item = &v["items"][0];
    let mut names: Vec<&str> = item.as_object().unwrap().keys().map(String::as_str).collect();
    names.sort();
    assert_eq!(names, ["saved_at", "subject_name", "target_id", "target_type", "teacher_name", "title", "updated_at"]);
    assert_eq!((item["target_type"].clone(), item["target_id"].clone(), item["title"].clone(), item["subject_name"].clone(), item["teacher_name"].clone()), (json!("course"), json!("c1"), json!("دورة بايثون"), json!("برمجة"), json!("المعلم")));
    // errors
    let (st, _, body) = put("me", "live", "l1").await;
    assert_eq!((st, body.contains("invalid_target")), (StatusCode::BAD_REQUEST, true));
    let (st, _, body) = put("me", "course", "nope").await;
    assert_eq!((st, body.contains("not_found")), (StatusCode::NOT_FOUND, true));
    h.db().execute("UPDATE courses SET status = 'draft'", []).unwrap();
    assert_eq!(put("outsider", "course", "c1").await.0, StatusCode::NOT_FOUND, "a hidden target cannot be saved");
    assert_eq!(get_json(&h, "/api/platform/my/bookmarks", "me").await.1["items"], json!([]), "and a hidden one is not listed");
    // removing always works
    assert_eq!(send(&h, Method::DELETE, "/api/platform/bookmarks/course/c1", Some("me"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&h, Method::DELETE, "/api/platform/bookmarks/course/c1", Some("me"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&h, Method::DELETE, "/api/platform/bookmarks/whatever/x", Some("me"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(h.db().query_row::<i64, _, _>("SELECT count(*) FROM bookmarks", [], |r| r.get(0)).unwrap(), 0);
    // the cap: 200 saved items that are shown; one more is a 409
    h.db().execute("UPDATE courses SET status = 'published'", []).unwrap();
    {
        let db = h.db();
        for n in 0..200 {
            db.execute(
                "INSERT INTO posts(id,teacher_id,subject_id,kind,title,body,status,created_at,updated_at) VALUES (?1,?2,'s1','article','م','ن','published',0,0)",
                params![format!("p{n}"), h.u.teacher.id],
            )
            .unwrap();
            db.execute("INSERT INTO bookmarks(user_id,target_type,target_id,created_at) VALUES (?1,'post',?2,?3)", params![h.u.me.id, format!("p{n}"), n]).unwrap();
        }
    }
    let (st, _, body) = put("me", "course", "c1").await;
    assert_eq!((st, body.contains("too_many_bookmarks")), (StatusCode::CONFLICT, true));
    assert_eq!(put("me", "post", "p7").await.0, StatusCode::NO_CONTENT, "what is saved already is not 'one more'");
    assert_eq!(put("follower", "course", "c1").await.0, StatusCode::NO_CONTENT, "the cap is per student");
    assert_eq!(get_json(&h, "/api/platform/my/bookmarks", "me").await.1["items"].as_array().unwrap().len(), 200);
    // mine are mine
    assert_eq!(get_json(&h, "/api/platform/my/bookmarks", "follower").await.1["items"].as_array().unwrap().len(), 1);
}
