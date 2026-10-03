mod platform;
mod platform_admin;
mod platform_learning;
mod relay;
mod routes;

use axum::{
    routing::{delete, get, post},
    Router,
};
use std::sync::{Arc, Mutex};
use routes::AppState;
use exameow_core::config::ConfigStore;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    let config_store = ConfigStore::new("ExameowServer").unwrap_or_else(|_| {
        eprintln!("Warning: could not init config store, using transient store");
        ConfigStore::new("ExameowServerTransient").unwrap()
    });
    let db_path = std::env::var("EXAM_DB_PATH").unwrap_or_else(|_| "./exameow.db".to_string());
    let relay = relay::init_db(&db_path).unwrap_or_else(|e| panic!("failed to init exam db at {db_path}: {e}"));
    let platform_db_path =
        std::env::var("PLATFORM_DB_PATH").unwrap_or_else(|_| "./exameow-platform.db".to_string());
    let platform = platform::init_db(&platform_db_path)
        .unwrap_or_else(|e| panic!("failed to init platform db at {platform_db_path}: {e}"));
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
    });

    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                relay::cleanup_expired(&state.relay);
                platform::cleanup_expired(&state.platform);
            }
        });
    }

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
        .fallback_service(ServeDir::new(&static_dir))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();
    println!("Exameow server running on http://0.0.0.0:{port}");
    axum::serve(listener, app).await.unwrap();
}
