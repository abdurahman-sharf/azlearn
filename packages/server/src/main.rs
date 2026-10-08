mod auth_gate;
mod client_ip;
mod legacy_guard;
mod platform;
mod platform_admin;
mod platform_ai;
mod platform_backup;
mod platform_bank;
mod platform_exam_admin;
mod platform_grading;
mod platform_content;
mod platform_engage;
mod platform_exams;
mod platform_ops;
mod platform_public;
mod platform_reminders;
mod platform_settings;
mod platform_stats;
mod platform_learning;
mod relay;
mod routes;
mod security_headers;
mod token_limit;

use axum::{
    routing::{delete, get, post},
    Router,
};
use std::sync::{Arc, Mutex};
use routes::AppState;
use exameow_core::config::ConfigStore;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use axum::http::{header, HeaderValue};

#[tokio::main]
async fn main() {
    // `exameow-server restore <backup.zip> [--yes]`: offline restore, then exit (the server must be stopped)
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("restore") {
        std::process::exit(platform_backup::cli_restore(&args[1..]));
    }
    let config_store = ConfigStore::new("ExameowServer").unwrap_or_else(|_| {
        eprintln!("Warning: could not init config store, using transient store");
        ConfigStore::new("ExameowServerTransient").unwrap()
    });
    let db_path = std::env::var("EXAM_DB_PATH").unwrap_or_else(|_| "./exameow.db".to_string());
    let relay = relay::init_db(&db_path).unwrap_or_else(|e| panic!("failed to init exam db at {db_path}: {e}"));
    let layout = platform_backup::Layout::from_env();
    let platform = platform::init_db(&layout)
        .unwrap_or_else(|e| panic!("failed to init platform db at {}: {e}", layout.db_path.display()));
    platform_backup::startup(&platform);
    let non_empty = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    match (non_empty("PLATFORM_ADMIN_EMAIL"), non_empty("PLATFORM_ADMIN_PASSWORD")) {
        (Some(email), Some(password)) => match platform::bootstrap_admin(&platform, &email, &password) {
            Ok(true) => println!("Platform: created the first admin account ({email})"),
            Ok(false) => {}
            Err(e) => eprintln!("Platform: admin bootstrap skipped: {e}"),
        },
        _ => println!("Platform: set PLATFORM_ADMIN_EMAIL and PLATFORM_ADMIN_PASSWORD to create the first admin"),
    }
    let admin_token = relay::load_admin_token();
    if admin_token == "pass" {
        println!("WARNING: ADMIN_TOKEN is the default \"pass\" — change it at /#/admin before exposing this server");
    }

    let state = Arc::new(AppState {
        config_store,
        relay,
        platform,
        admin_token: Mutex::new(admin_token),
        legacy: legacy_guard::LegacyGuard::from_env(),
        auth_failures: token_limit::TokenLimiter::from_env_named("PLATFORM_AUTH_FAIL_RPM", 600),
    });

    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                relay::cleanup_expired(&state.relay);
                platform::cleanup_expired(&state.platform);
                platform_content::purge_orphan_files(&state.platform);
                platform_engage::purge_old_notifications(&state.platform);
                platform_exams::settle_overdue(&state.platform);
                // tonight's backup (phase 1-9): also fires on the first tick after a start if the last slot was missed
                let st = state.clone();
                let _ = tokio::task::spawn_blocking(move || platform_backup::run_nightly(&st.platform, relay::now_ms())).await;
            }
        });
    }

    // Exam reminders and "results available" notifications (phase 1-8). The first tick fires at once, so a
    // server that was stopped catches up on start; a missed tick is skipped, never replayed in a burst.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let state = state.clone();
                // SQLite work: keep it off the async workers
                let _ = tokio::task::spawn_blocking(move || platform_reminders::run(&state.platform)).await;
            }
        });
    }

    let proxy = client_ip::Proxy::from_env();
    let sec = security_headers::Config::from_env();
    println!("Content-Security-Policy: {:?}, HSTS: {}", sec.csp, sec.hsts);
    let token_limiter = Arc::new(token_limit::TokenLimiter::from_env());
    println!("Signed-in API budget: {} requests/minute per session (PLATFORM_TOKEN_RPM, 0 = off)", token_limiter.per_minute());
    println!("Client address: trusted proxy hops = {}", proxy.describe());

    let static_dir =
        std::env::var("STATIC_DIR").unwrap_or_else(|_| "../frontend/dist".to_string());

    let app = Router::new()
        .route("/api/models", get(routes::get_models))
        .route("/api/generate", post(routes::generate_exam_handler))
        .route("/api/answer", post(routes::answer_handler))
        .route("/api/judge", post(routes::judge_handler))
        .route("/api/explain", post(routes::explain_handler))
        .route("/api/export", get(routes::export_handler))
        .route("/api/export/xlsx", post(routes::export_xlsx_handler))
        .route("/api/config/save", post(routes::save_config_handler))
        .route("/api/config/load", get(routes::load_config_handler))
        .route("/api/config/server", get(routes::server_config_info_handler))
        .route("/api/exam/publish", post(relay::publish_handler))
        .route("/api/exam/code/{code}", get(relay::get_exam_handler).delete(relay::delete_exam_handler))
        .route("/api/exam/code/{code}/submit", post(relay::submit_handler))
        .route("/api/exam/code/{code}/results", get(relay::results_handler))
        .route("/api/exam/code/{code}/report", post(relay::report_handler))
        .route("/api/exam/admin/reports", get(relay::admin_reports_handler))
        .route("/api/exam/admin/code/{code}", delete(relay::admin_delete_handler))
        .route("/api/exam/admin/code/{code}/restore", post(relay::admin_restore_handler))
        .route("/api/exam/admin/token", post(relay::admin_change_token_handler))
        .route("/api/platform/register", post(platform::register_handler))
        .route("/api/platform/login", post(platform::login_handler))
        .route("/api/platform/logout", post(platform::logout_handler))
        .route("/api/platform/me", get(platform::me_handler))
        .route("/api/platform/institutions", get(platform_admin::list_institutions_handler))
        .route("/api/platform/institutions/{id}/structure", get(platform_admin::structure_handler))
        .route("/api/platform/admin/users", get(platform_admin::list_users_handler))
        .route("/api/platform/admin/users/{id}", axum::routing::patch(platform_admin::update_user_handler))
        .route("/api/platform/admin/institutions", post(platform_admin::create_institution_handler))
        .route(
            "/api/platform/admin/institutions/{id}",
            axum::routing::patch(platform_admin::update_institution_handler).delete(platform_admin::delete_institution_handler),
        )
        .route("/api/platform/admin/units", post(platform_admin::create_unit_handler))
        .route(
            "/api/platform/admin/units/{id}",
            axum::routing::patch(platform_admin::update_unit_handler).delete(platform_admin::delete_unit_handler),
        )
        .route("/api/platform/admin/subjects", post(platform_admin::create_subject_handler))
        .route(
            "/api/platform/admin/subjects/{id}",
            axum::routing::patch(platform_admin::update_subject_handler).delete(platform_admin::delete_subject_handler),
        )
        .route("/api/platform/me/profile", axum::routing::patch(platform_learning::update_profile_handler))
        .route("/api/platform/me/placement", get(platform_learning::get_placement_handler).put(platform_learning::set_placement_handler))
        .route("/api/platform/teaching", get(platform_learning::my_teaching_handler).post(platform_learning::request_teaching_handler))
        .route("/api/platform/teaching/{subject_id}", delete(platform_learning::drop_teaching_handler))
        .route("/api/platform/admin/teaching", get(platform_learning::admin_teaching_handler).patch(platform_learning::admin_decide_teaching_handler))
        .route("/api/platform/enrollments", get(platform_learning::my_enrollments_handler).post(platform_learning::enroll_handler))
        .route("/api/platform/enrollments/{subject_id}", delete(platform_learning::unenroll_handler))
        .route("/api/platform/teachers", get(platform_learning::list_teachers_handler))
        .route("/api/platform/teachers/{id}", get(platform_learning::teacher_page_handler))
        .route("/api/platform/teachers/{id}/follow", post(platform_learning::follow_handler).delete(platform_learning::unfollow_handler))
        .route("/api/platform/subjects/{id}", get(platform_learning::subject_page_handler))
        .route("/api/platform/posts", post(platform_content::create_post_handler))
        .route(
            "/api/platform/posts/{id}",
            get(platform_content::get_post_handler).patch(platform_content::update_post_handler).delete(platform_content::delete_post_handler),
        )
        .route(
            "/api/platform/posts/{id}/file",
            post(platform_content::upload_file_handler).delete(platform_content::remove_file_handler).layer(platform_content::upload_limit()),
        )
        .route("/api/platform/files/{id}", get(platform_content::download_file_handler))
        .route("/api/platform/courses", post(platform_content::create_course_handler))
        .route(
            "/api/platform/courses/{id}",
            get(platform_content::get_course_handler).patch(platform_content::update_course_handler).delete(platform_content::delete_course_handler),
        )
        .route("/api/platform/courses/{id}/lessons", post(platform_content::add_lesson_handler))
        .route(
            "/api/platform/lessons/{id}",
            axum::routing::patch(platform_content::update_lesson_handler).delete(platform_content::delete_lesson_handler),
        )
        .route("/api/platform/lessons/{id}/move", post(platform_content::move_lesson_handler))
        .route("/api/platform/live", post(platform_content::create_live_handler))
        .route(
            "/api/platform/live/{id}",
            axum::routing::patch(platform_content::update_live_handler).delete(platform_content::delete_live_handler),
        )
        .route("/api/platform/subjects/{id}/content", get(platform_content::subject_content_handler))
        .route("/api/platform/teachers/{id}/content", get(platform_content::teacher_content_handler))
        .route("/api/platform/my/content", get(platform_content::my_content_handler))
        .route("/api/platform/feed", get(platform_content::feed_handler))
        .route("/api/platform/notifications", get(platform_engage::notifications_handler))
        .route("/api/platform/notifications/read", post(platform_engage::mark_read_handler))
        .route("/api/platform/lessons/{id}/complete", axum::routing::put(platform_engage::complete_handler).delete(platform_engage::uncomplete_handler))
        .route("/api/platform/courses/{id}/progress", get(platform_engage::course_progress_handler))
        .route("/api/platform/my/progress", get(platform_engage::my_progress_handler))
        .route("/api/platform/reviews", axum::routing::put(platform_engage::put_review_handler))
        .route("/api/platform/reviews/{target_type}/{id}", get(platform_engage::reviews_handler))
        .route("/api/platform/reviews/{id}", delete(platform_engage::delete_review_handler))
        .route("/api/platform/assessments", post(platform_exams::create_handler))
        .route("/api/platform/assessments/mine", get(platform_exams::mine_handler))
        .route("/api/platform/assessments/available", get(platform_exams::available_handler))
        .route(
            "/api/platform/assessments/{id}",
            get(platform_exams::detail_handler).patch(platform_exams::update_handler).delete(platform_exams::delete_handler),
        )
        .route("/api/platform/assessments/{id}/start", post(platform_exams::start_handler))
        .route("/api/platform/assessments/{id}/results", get(platform_exams::results_handler))
        .route("/api/platform/subjects/{id}/assessments", get(platform_exams::subject_list_handler))
        .route("/api/platform/my/attempts", get(platform_exams::my_attempts_handler))
        .route("/api/platform/attempts/{id}", get(platform_exams::attempt_handler))
        .route("/api/platform/attempts/{id}/submit", post(platform_exams::submit_handler))
        .route("/api/platform/attempts/{id}/grade", axum::routing::patch(platform_exams::grade_handler))
        .route("/api/platform/reports", post(platform_ops::report_handler))
        .route("/api/platform/admin/reports", get(platform_ops::list_reports_handler))
        .route("/api/platform/admin/reports/{id}", axum::routing::patch(platform_ops::resolve_report_handler))
        .route("/api/platform/admin/stats", get(platform_ops::stats_handler))
        .route("/api/platform/admin/stats/institutions", get(platform_stats::institutions_stats_handler))
        .route("/api/platform/admin/stats/subjects", get(platform_stats::subjects_stats_handler))
        .route("/api/platform/admin/backup", post(platform_backup::create_handler))
        .route("/api/platform/admin/backups", get(platform_backup::list_handler))
        .route("/api/platform/admin/backups/{id}", get(platform_backup::download_handler))
        .route("/api/platform/admin/system", get(platform_backup::system_handler))
        .route("/api/platform/admin/audit", get(platform_ops::audit_handler))
        .route("/api/platform/search", get(platform_ops::search_handler))
        .route("/api/platform/me/password", post(platform_ops::change_password_handler))
        .route("/api/platform/me", delete(platform_ops::delete_account_handler))
        .route("/api/platform/public/config", get(platform_public::public_config_handler))
        .route("/api/platform/public/logo", get(platform_public::logo_handler))
        .route("/api/platform/public/stats", get(platform_stats::public_stats_handler))
        .route("/api/platform/public/legal/{slug}", get(platform_public::legal_get_handler))
        .route("/api/platform/admin/settings", get(platform_ai::get_settings_handler).put(platform_ai::put_settings_handler))
        .route("/api/platform/admin/settings/ai/test", post(platform_ai::test_handler))
        .route("/api/platform/admin/ai/usage", get(platform_ai::usage_handler))
        .route("/api/platform/admin/ai/generate", post(platform_ai::generate_handler).layer(platform_ai::upload_limit()))
        .route("/api/platform/admin/bank", get(platform_bank::list_handler).post(platform_bank::create_handler))
        .route("/api/platform/admin/bank/facets", get(platform_bank::facets_handler))
        .route("/api/platform/admin/bank/bulk", post(platform_bank::bulk_handler))
        .route("/api/platform/admin/bank/import", post(platform_bank::import_handler).layer(platform_bank::import_limit()))
        .route("/api/platform/admin/bank/{id}", axum::routing::patch(platform_bank::update_handler))
        .route("/api/platform/admin/exams", get(platform_exam_admin::list_handler).post(platform_exam_admin::create_handler))
        .route("/api/platform/admin/exams/{id}", get(platform_exam_admin::get_handler).patch(platform_exam_admin::update_handler).delete(platform_exam_admin::delete_handler))
        .route("/api/platform/admin/exams/{id}/{action}", post(platform_exam_admin::action_handler))
        .route("/api/platform/attempts/{id}/answers", axum::routing::put(platform_exams::save_handler))
        .route("/api/platform/attempts/{id}/events", post(platform_exams::event_handler))
        .route("/api/platform/grading/pending", get(platform_grading::pending_handler))
        .route("/api/platform/assessments/{id}/grading", get(platform_grading::sheet_handler))
        .route("/api/platform/assessments/{id}/grade-batch", post(platform_grading::batch_handler))
        .route("/api/platform/assessments/{id}/analytics", get(platform_grading::analytics_handler))
        .route("/api/platform/assessments/{id}/attempts", get(platform_grading::table_handler))
        .route("/api/platform/assessments/{id}/export", get(platform_grading::export_handler))
        .route("/api/platform/admin/branding", axum::routing::put(platform_public::set_branding_handler))
        .route(
            "/api/platform/admin/branding/logo",
            post(platform_public::upload_logo_handler).delete(platform_public::delete_logo_handler).layer(platform_public::logo_upload_limit()),
        )
        .route("/api/platform/admin/legal/{slug}", axum::routing::put(platform_public::legal_put_handler))
        .fallback_service(ServeDir::new(&static_dir))
        // both sit inside CORS and the security headers on purpose: their 401/429 answers still carry those headers,
        // so cross-origin clients (Tauri/Cloudflare builds) can read them
        // deny by default: no valid session, no platform API (except register/login/logout/public)
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth_gate::middleware))
        // outside the gate: a token over its budget (valid or made up) is refused before any database lookup
        .layer(axum::middleware::from_fn_with_state(token_limiter, token_limit::middleware))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        // CSP / no-store API / permissions policy / optional HSTS (see security_headers.rs)
        .layer(axum::middleware::from_fn_with_state(sec, security_headers::middleware))
        // baseline headers
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("strict-origin-when-cross-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN")))
        .with_state(state)
        // outermost: decides the client address before any handler (or rate limit) looks at the request
        .layer(axum::middleware::from_fn_with_state(proxy, client_ip::middleware));

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();
    println!("Exameow server running on http://0.0.0.0:{port}");
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
}
