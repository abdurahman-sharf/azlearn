//! Phase 4-3 over real requests (a `Router` driven with `oneshot`): the server clock in the start / save / exam-page answers,
//! partial autosave, the practice route and the statistics route - their role walls, shapes, limits and the promise that
//! practice writes nothing.

use super::fixtures::*;
use super::*;
use crate::platform::{insert_test_user, new_id, PlatformState};
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request};
use axum::routing::{get, post, put};
use axum::Router;
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

/// The fixture world plus a student outside the subject, a pending teacher and a suspended student, a session for each,
/// and the router (the same registrations as `main.rs`, so a route conflict would panic here).
fn harness() -> H {
    static CFG: std::sync::Once = std::sync::Once::new();
    CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-exam-http-test-{}", new_id()))));
    let w = world();
    let outsider = insert_test_user(&w.conn, "out@x.com", "student", "active");
    let pending = insert_test_user(&w.conn, "pend@x.com", "teacher", "pending");
    let suspended = insert_test_user(&w.conn, "sus@x.com", "student", "suspended");
    w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![suspended.id]).unwrap();
    let mut tokens = HashMap::new();
    let mut ids = HashMap::new();
    for (key, u) in [("student", &w.student), ("outsider", &outsider), ("teacher", &w.teacher), ("admin", &w.admin), ("pending", &pending), ("suspended", &suspended)] {
        tokens.insert(key, token_for(&w.conn, u));
        ids.insert(key, u.id.clone());
    }
    let state = Arc::new(AppState {
        config_store: exameow_core::config::ConfigStore::new("ExameowServerHttpTest").expect("a scratch config dir"),
        relay: crate::relay::init_db(":memory:").unwrap(),
        platform: PlatformState::for_tests(w.conn),
        admin_token: std::sync::Mutex::new("pass".into()),
        legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
        auth_failures: crate::token_limit::TokenLimiter::new(600),
    });
    let app = Router::new()
        .route("/api/platform/assessments/available", get(available_handler))
        .route("/api/platform/assessments/{id}", get(detail_handler))
        .route("/api/platform/assessments/{id}/start", post(start_handler))
        .route("/api/platform/attempts/{id}", get(attempt_handler))
        .route("/api/platform/attempts/{id}/submit", post(submit_handler))
        .route("/api/platform/attempts/{id}/answers", put(save_handler))
        .merge(crate::platform_practice::routes())
        .merge(crate::platform_student_stats::routes())
        .with_state(state.clone());
    H { app, state, tokens, ids }
}

impl H {
    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.state.platform.conn.lock().unwrap()
    }

    /// An admin exam created in real time (the handlers read the real clock), `extra` merged over the fixture's defaults.
    fn make_exam(&self, extra: Value) -> String {
        let admin = User { id: self.ids["admin"].clone(), email: String::new(), full_name: "T".into(), role: "admin".into(), institution_type: None, status: "active".into(), status_reason: None };
        crate::platform_exam_admin::create_exam(&self.db(), &admin, &exam_req(extra), now_ms()).unwrap().info_id()
    }
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

// ───────── the server clock ─────────

#[tokio::test]
async fn starting_an_exam_answers_with_the_servers_clock_and_what_caps_the_attempt() {
    let h = harness();
    let closes = now_ms() + 10 * MIN;
    let id = h.make_exam(json!({"duration_min": 60, "closes_at": closes}));
    let (before, (st, v)) = (now_ms(), call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await);
    let after = now_ms();
    assert_eq!(st, StatusCode::OK, "{v}");
    let server_now = v["server_now"].as_i64().expect("server_now is a number of milliseconds");
    assert!(before <= server_now && server_now <= after, "{before} <= {server_now} <= {after}");
    assert_eq!((v["ends_at"].as_i64(), v["limited_by_closing"].clone(), v["resumed"].clone()), (Some(closes), json!(true), json!(false)));
    assert!(v["attempt_id"].is_string() && v["started_at"].is_i64() && v["questions"].as_array().map(Vec::len) == Some(5));
    // the exam page for the same student: the clock, the switch, and the running attempt
    let (_, d) = call(&h, Method::GET, &format!("/api/platform/assessments/{id}"), Some("student"), None).await;
    assert!(d["server_now"].as_i64().unwrap() >= server_now, "{d}");
    assert_eq!((d["allow_practice"].clone(), d["in_progress_attempt"].clone(), d["state"].clone()), (json!(false), v["attempt_id"].clone(), json!("in_progress")));
    // resuming says the same about the end
    let (_, r) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    assert_eq!((r["resumed"].clone(), r["ends_at"].as_i64(), r["limited_by_closing"].clone()), (json!(true), Some(closes), json!(true)));
    // a short exam inside a long window is not "limited by closing"
    let short = h.make_exam(json!({"title": "قصير", "duration_min": 5, "closes_at": closes}));
    let (_, s) = call(&h, Method::POST, &format!("/api/platform/assessments/{short}/start"), Some("student"), None).await;
    assert_eq!(s["limited_by_closing"], json!(false));
    assert_eq!(s["ends_at"].as_i64().unwrap() - s["started_at"].as_i64().unwrap(), 5 * MIN);
    // staff get no clock on the exam page
    for who in ["teacher", "admin"] {
        let (_, d) = call(&h, Method::GET, &format!("/api/platform/assessments/{id}"), Some(who), None).await;
        assert!(d["server_now"].is_null(), "{who}: {d}");
    }
}

// ───────── partial autosave over HTTP ─────────

#[tokio::test]
async fn the_autosave_route_merges_by_default_replaces_on_request_and_reports_the_clock() {
    let h = harness();
    let id = h.make_exam(json!({}));
    let (_, s) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    let url = format!("/api/platform/attempts/{}/answers", s["attempt_id"].as_str().unwrap());
    let (before, (st, v)) = (now_ms(), call(&h, Method::PUT, &url, Some("student"), Some(json!({"answers": {"q1": "B"}}))).await);
    assert_eq!(st, StatusCode::OK, "{v}");
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["answered", "ends_at", "limited_by_closing", "saved_at", "server_now"]);
    assert_eq!((v["ends_at"].clone(), v["limited_by_closing"].clone()), (s["ends_at"].clone(), json!(false)), "the end the page was handed, unless the owner moved it");
    assert!(v["saved_at"].as_i64().unwrap() >= before && v["server_now"].as_i64().unwrap() >= v["saved_at"].as_i64().unwrap(), "{v}");
    assert_eq!(v["answered"], json!(1));
    // an old client sends no `replace`: that is a merge now
    let (_, v) = call(&h, Method::PUT, &url, Some("student"), Some(json!({"answers": {"q2": "AC"}}))).await;
    assert_eq!(v["answered"], json!(2), "two partial saves = their union");
    let (_, v) = call(&h, Method::PUT, &url, Some("student"), Some(json!({"answers": {"q1": ""}}))).await;
    assert_eq!(v["answered"], json!(1), "an empty string removes that answer");
    let (_, v) = call(&h, Method::PUT, &url, Some("student"), Some(json!({"answers": {"q4": "مصر", "q3": "A"}, "replace": true}))).await;
    assert_eq!(v["answered"], json!(2), "replace: the request is the whole sheet");
    let (_, r) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    assert_eq!(r["saved_answers"], json!({"q3": "A", "q4": "مصر"}));
    // errors keep their codes
    let (st, e) = call(&h, Method::PUT, &url, Some("outsider"), Some(json!({"answers": {}}))).await;
    assert_eq!((st, e["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "someone else's attempt");
    let (st, e) = call(&h, Method::PUT, &url, Some("teacher"), Some(json!({"answers": {}}))).await;
    assert_eq!((st, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "only students save");
    let (st, _) = call(&h, Method::PUT, &url, None, Some(json!({"answers": {}}))).await;
    assert!(st == StatusCode::UNAUTHORIZED || st == StatusCode::FORBIDDEN, "no session: {st}");
    let (st, _) = call(&h, Method::POST, &format!("/api/platform/attempts/{}/submit", s["attempt_id"].as_str().unwrap()), Some("student"), Some(json!({"answers": {"q3": "A"}}))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, e) = call(&h, Method::PUT, &url, Some("student"), Some(json!({"answers": {"q1": "A"}}))).await;
    assert_eq!((st, e["error"].as_str()), (StatusCode::CONFLICT, Some("already_submitted")));
}

// ───────── the review over HTTP ─────────

#[tokio::test]
async fn the_review_route_carries_the_new_fields() {
    let h = harness();
    let id = h.make_exam(json!({}));
    let (_, s) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    let attempt = s["attempt_id"].as_str().unwrap().to_string();
    let (st, r) = call(&h, Method::POST, &format!("/api/platform/attempts/{attempt}/submit"), Some("student"), Some(json!({"answers": {"q1": "B", "q2": "AC", "q3": "A", "q4": "مصر"}}))).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    for who in ["student", "admin"] {
        let (_, r) = call(&h, Method::GET, &format!("/api/platform/attempts/{attempt}"), Some(who), None).await;
        assert_eq!((r["attempt_no"].clone(), r["is_best"].clone()), (json!(1), json!(true)), "{who}");
        assert!(r["duration_sec"].is_i64() && r["started_at"].is_i64() && r["submitted_at"].is_i64(), "{who}: {r}");
        let item = &r["items"][1];
        assert_eq!((item["your_answer"].clone(), item["your_answer_text"].clone(), item["chapter"].clone(), item["difficulty"].clone()), (json!("AC"), json!("o2-0 | o2-2"), json!("ف1"), json!("medium")), "{who}");
        assert!(r["items"][4]["your_answer_text"].is_null(), "{who}: the written question was left blank");
    }
}

// ───────── practice over HTTP ─────────

#[tokio::test]
async fn only_an_active_enrolled_student_reaches_the_practice_route() {
    let h = harness();
    let id = h.make_exam(json!({"allow_practice": true}));
    let url = format!("/api/platform/assessments/{id}/practice");
    for who in ["teacher", "admin", "pending", "suspended"] {
        let (st, v) = call(&h, Method::GET, &url, Some(who), None).await;
        assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{who}");
    }
    let (st, _) = call(&h, Method::GET, &url, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "no session");
    let (st, v) = call(&h, Method::GET, &url, Some("outsider"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_enrolled")));
    let (st, v) = call(&h, Method::GET, "/api/platform/assessments/ghost/practice", Some("student"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
    let (st, v) = call(&h, Method::GET, &url, Some("student"), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["question_count"].clone(), v["total_points"].clone(), v["questions"][3]["answer"].clone()), (json!(5), json!(9.0), json!("القاهرة|مصر")));
    // switched off: an explanation, not a leak
    let off = h.make_exam(json!({"title": "بلا تدريب"}));
    let (st, v) = call(&h, Method::GET, &format!("/api/platform/assessments/{off}/practice"), Some("student"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("practice_disabled")));
    assert!(!v.to_string().contains("القاهرة"), "{v}");
}

#[tokio::test]
async fn the_practice_route_writes_nothing_and_stops_a_runaway_client_at_thirty_a_minute() {
    let h = harness();
    let id = h.make_exam(json!({"allow_practice": true}));
    let url = format!("/api/platform/assessments/{id}/practice");
    // `rate_limits` is the request counter itself; every other table must be exactly as it was
    let without_counter = |s: String| s.lines().filter(|l| !l.starts_with("rate_limits|")).collect::<Vec<_>>().join("\n");
    let before = without_counter(dump_all(&h.db()));
    let mut ok = 0;
    let mut limited = None;
    // (a run of requests that straddles a minute boundary meets a fresh window, so ask for more than two windows' worth)
    for i in 1..=65 {
        let (st, v) = call(&h, Method::GET, &url, Some("student"), None).await;
        if st == StatusCode::OK {
            ok += 1;
        } else {
            limited = Some((i, st, v["error"].as_str().map(String::from)));
            break;
        }
    }
    assert!(ok >= 1, "the first request is fine");
    let (at, st, error) = limited.expect("a client that asks and asks is stopped");
    assert_eq!((st, error.as_deref()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
    assert!(at <= 61 && ok <= 60, "stopped after at most one window's thirty (or two across a minute boundary): ok={ok}");
    assert_eq!(without_counter(dump_all(&h.db())), before, "no attempt, audit row, notification, marker or statistic: not one other row changed");
    // another student has their own budget
    let (st, _) = call(&h, Method::GET, &url, Some("suspended"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a different user is judged by their own role, not by the first student's counter");
}

// ───────── statistics over HTTP ─────────

#[tokio::test]
async fn only_an_active_student_reaches_the_statistics_route_and_gets_the_agreed_shape() {
    let h = harness();
    for who in ["teacher", "admin", "pending", "suspended"] {
        let (st, v) = call(&h, Method::GET, "/api/platform/my/stats", Some(who), None).await;
        assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{who}");
    }
    let (st, _) = call(&h, Method::GET, "/api/platform/my/stats", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, v) = call(&h, Method::GET, "/api/platform/my/stats", Some("student"), None).await;
    assert_eq!((st, v), (StatusCode::OK, json!({"overall": {"attempts": 0, "exams": 0, "average_percent": null}, "subjects": []})));
    // one fully graded attempt
    let id = h.make_exam(json!({}));
    let (_, s) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    call(&h, Method::POST, &format!("/api/platform/attempts/{}/submit", s["attempt_id"].as_str().unwrap()), Some("student"), Some(json!({"answers": {"q1": "B", "q2": "AC", "q3": "A", "q4": "مصر"}}))).await;
    let (_, v) = call(&h, Method::GET, "/api/platform/my/stats", Some("student"), None).await;
    assert_eq!((v["overall"]["attempts"].clone(), v["overall"]["exams"].clone(), v["overall"]["average_percent"].clone()), (json!(1), json!(1), json!(55.56)));
    let subject = &v["subjects"][0];
    assert_eq!((subject["subject_id"].clone(), subject["subject_name"].clone(), subject["exams"].clone()), (json!("s1"), json!("برمجة"), json!(1)));
    assert_eq!(subject["trend"][0]["exam_title"], json!("امتحان"));
    assert_eq!(subject["weakest"], json!([]));
    // another student's page is their own
    let (_, other) = call(&h, Method::GET, "/api/platform/my/stats", Some("outsider"), None).await;
    assert_eq!(other["overall"]["attempts"], json!(0));
}

#[tokio::test]
async fn the_statistics_route_stops_a_reload_loop_at_thirty_a_minute() {
    let h = harness();
    let mut limited = None;
    for i in 1..=70 {
        let (st, v) = call(&h, Method::GET, "/api/platform/my/stats", Some("student"), None).await;
        if st != StatusCode::OK {
            limited = Some((i, st, v["error"].as_str().map(String::from)));
            break;
        }
    }
    let (at, st, error) = limited.expect("a client that reloads and reloads is stopped");
    assert_eq!((st, error.as_deref()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
    assert!(at <= 61, "stopped after at most one window's thirty (or two across a minute boundary): {at}");
    let (st, _) = call(&h, Method::GET, "/api/platform/my/stats", Some("outsider"), None).await;
    assert_eq!(st, StatusCode::OK, "another student has their own budget");
}

#[tokio::test]
async fn the_practice_route_keeps_the_key_back_while_the_students_own_attempt_runs() {
    let h = harness();
    let id = h.make_exam(json!({"allow_practice": true}));
    let url = format!("/api/platform/assessments/{id}/practice");
    let (st, _) = call(&h, Method::GET, &url, Some("student"), None).await;
    assert_eq!(st, StatusCode::OK, "before they start");
    let (_, s) = call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await;
    let attempt = s["attempt_id"].as_str().unwrap().to_string();
    let (st, v) = call(&h, Method::GET, &url, Some("student"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::CONFLICT, Some("attempt_running")));
    assert!(!v.to_string().contains("القاهرة"), "no key in the refusal: {v}");
    call(&h, Method::POST, &format!("/api/platform/attempts/{attempt}/submit"), Some("student"), Some(json!({"answers": {}}))).await;
    let (st, _) = call(&h, Method::GET, &url, Some("student"), None).await;
    assert_eq!(st, StatusCode::OK, "after they hand it in");
    // an exam that opens later and one whose results are embargoed answer 403 practice_not_available
    let later = h.make_exam(json!({"title": "لاحق", "allow_practice": true, "opens_at": now_ms() + H}));
    let (st, v) = call(&h, Method::GET, &format!("/api/platform/assessments/{later}/practice"), Some("student"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("practice_not_available")));
    let embargo = h.make_exam(json!({"title": "مؤجل", "allow_practice": true, "release_mode": "after_close", "closes_at": now_ms() + 2 * H}));
    let (st, v) = call(&h, Method::GET, &format!("/api/platform/assessments/{embargo}/practice"), Some("student"), None).await;
    assert_eq!((st, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("practice_not_available")));
}

#[tokio::test]
async fn the_submit_route_lays_the_hand_in_over_the_autosave_unless_told_to_replace() {
    let h = harness();
    let id = h.make_exam(json!({"max_attempts": 2}));
    let start = || async { call(&h, Method::POST, &format!("/api/platform/assessments/{id}/start"), Some("student"), None).await.1["attempt_id"].as_str().unwrap().to_string() };
    let first = start().await;
    let save = format!("/api/platform/attempts/{first}/answers");
    call(&h, Method::PUT, &save, Some("student"), Some(json!({"answers": {"q1": "B", "q2": "AC"}}))).await;
    let (st, r) = call(&h, Method::POST, &format!("/api/platform/attempts/{first}/submit"), Some("student"), Some(json!({"answers": {"q3": "A"}}))).await;
    assert_eq!((st, r["score"].clone()), (StatusCode::OK, json!(4.0)), "q1 and q2 from the autosave, q3 from the hand-in: {r}");
    let second = start().await;
    call(&h, Method::PUT, &format!("/api/platform/attempts/{second}/answers"), Some("student"), Some(json!({"answers": {"q1": "B", "q2": "AC"}}))).await;
    let (st, r) = call(&h, Method::POST, &format!("/api/platform/attempts/{second}/submit"), Some("student"), Some(json!({"answers": {"q3": "A"}, "replace": true}))).await;
    assert_eq!((st, r["score"].clone()), (StatusCode::OK, json!(1.0)), "an old client that always sends everything: {r}");
}
