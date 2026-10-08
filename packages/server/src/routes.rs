use axum::{
    extract::{Multipart, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use exameow_core::ai::{AIClient, AIRequestOptions, ModelInfo};
use exameow_core::config::{AIConfigData, ConfigStore};
use exameow_core::exam::{
    answer_question, explain_question, generate_exam, judge_answer, AnswerResult, ExamParams, ExplainResult, JudgeResult, Question,
};
use exameow_core::parser::parse_file;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub config_store: ConfigStore,
    pub relay: crate::relay::RelayState,
    pub platform: crate::platform::PlatformState,
    pub admin_token: Mutex<String>,
    pub legacy: crate::legacy_guard::LegacyGuard,
}

#[derive(Deserialize)]
pub struct ModelsQuery {
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Serialize)]
pub struct GenerateResult {
    pub questions: Vec<Question>,
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub questions: String,
}

fn ai_endpoint() -> String { std::env::var("AI_ENDPOINT").unwrap_or_default() }
fn ai_api_key() -> String { std::env::var("AI_API_KEY").unwrap_or_default() }
fn ai_model() -> String { std::env::var("AI_MODEL").unwrap_or_default() }

/// The AI connection a legacy request will use.
pub struct Resolved {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    /// The caller sent no key, so the server's `AI_API_KEY` pays for this call.
    pub used_server_key: bool,
    /// The caller named an endpoint other than the operator's `AI_ENDPOINT` (that one is trusted, this one is not).
    pub custom_endpoint: bool,
}

fn same_endpoint(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/').eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

/// Merges what the caller sent with the server's `AI_*` environment. The server's key is only ever used with the
/// server's own endpoint — a caller-chosen endpoint without a caller key is refused, never sent the operator's key.
fn resolve_ai(endpoint: Option<&str>, api_key: Option<&str>, model: Option<&str>) -> Result<Resolved, (StatusCode, String)> {
    resolve_with((ai_endpoint(), ai_api_key(), ai_model()), endpoint, api_key, model)
}

fn resolve_with(
    (server_endpoint, server_key, server_model): (String, String, String),
    endpoint: Option<&str>,
    api_key: Option<&str>,
    model: Option<&str>,
) -> Result<Resolved, (StatusCode, String)> {
    let no_config = || (StatusCode::BAD_REQUEST, "No AI config (set AI_ENDPOINT/AI_API_KEY env vars)".to_string());
    let given = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let endpoint = given(endpoint).unwrap_or_else(|| server_endpoint.clone());
    let (api_key, used_server_key) = match given(api_key) {
        Some(k) => (k, false),
        None if !server_endpoint.is_empty() && same_endpoint(&endpoint, &server_endpoint) => (server_key, true),
        None => return Err(no_config()),
    };
    if endpoint.is_empty() || api_key.is_empty() {
        return Err(no_config());
    }
    let model = given(model).unwrap_or(server_model);
    let custom_endpoint = server_endpoint.is_empty() || !same_endpoint(&endpoint, &server_endpoint);
    Ok(Resolved { endpoint, api_key, model, used_server_key, custom_endpoint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> (String, String, String) {
        ("https://api.example.com/v1".into(), "sk-server-secret".into(), "server-model".into())
    }

    #[test]
    fn the_servers_key_never_goes_to_an_endpoint_the_caller_chose() {
        // the exfiltration case: caller names its own endpoint and sends no key
        assert!(resolve_with(server(), Some("https://evil.example.org/v1"), None, None).is_err());
        assert!(resolve_with(server(), Some("https://evil.example.org/v1"), Some(""), Some("m")).is_err());
        // a look-alike of the server's endpoint is a different endpoint
        assert!(resolve_with(server(), Some("https://api.example.com.evil.org/v1"), None, None).is_err());
        assert!(resolve_with(server(), Some("https://api.example.com/v1/x"), None, None).is_err());
    }

    #[test]
    fn caller_supplied_or_server_defaults_resolve_as_before() {
        let r = resolve_with(server(), None, None, None).unwrap();
        assert_eq!((r.endpoint.as_str(), r.api_key.as_str(), r.model.as_str(), r.used_server_key), ("https://api.example.com/v1", "sk-server-secret", "server-model", true));
        // naming the server's own endpoint (trailing slash / case differences) still counts as the server's
        let r = resolve_with(server(), Some(" HTTPS://API.example.com/v1/ "), Some(""), None).unwrap();
        assert!(r.used_server_key && r.api_key == "sk-server-secret");
        // the caller's own key + endpoint: the operator's key is untouched and the call is free for the operator
        let r = resolve_with(server(), Some("https://other.example.org/v1"), Some("sk-mine"), Some("gpt")).unwrap();
        assert_eq!((r.endpoint.as_str(), r.api_key.as_str(), r.model.as_str(), r.used_server_key), ("https://other.example.org/v1", "sk-mine", "gpt", false));
        // the caller's key with the server's endpoint
        let r = resolve_with(server(), None, Some("sk-mine"), None).unwrap();
        assert!(!r.used_server_key && r.api_key == "sk-mine" && r.endpoint == "https://api.example.com/v1");
        // only the operator's own endpoint is trusted; everything else the caller named is vetted
        assert!(!r.custom_endpoint);
        assert!(resolve_with(server(), Some("https://other.example.org/v1"), Some("k"), None).unwrap().custom_endpoint);
        assert!(resolve_with((String::new(), String::new(), String::new()), Some("https://x.example/v1"), Some("k"), None).unwrap().custom_endpoint);
    }

    #[test]
    fn nothing_configured_anywhere_is_a_clean_400() {
        let none = (String::new(), String::new(), String::new());
        assert_eq!(resolve_with(none.clone(), None, None, None).err().unwrap().0, StatusCode::BAD_REQUEST);
        assert_eq!(resolve_with(none, Some("https://x.example/v1"), None, None).err().unwrap().0, StatusCode::BAD_REQUEST);
        // a server key without an endpoint is not enough
        let key_only = (String::new(), "sk".to_string(), String::new());
        assert!(resolve_with(key_only, None, None, None).is_err());
    }
}

pub async fn get_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ModelsQuery>,
) -> Result<Json<Vec<ModelInfo>>, (StatusCode, String)> {
    let r = resolve_ai(params.endpoint.as_deref(), params.api_key.as_deref(), None)?;
    state.legacy.check(&headers, r.custom_endpoint.then_some(r.endpoint.as_str()), r.used_server_key).await?;

    let client = AIClient::new(&r.endpoint, &r.api_key);
    let models = client
        .fetch_models()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("AI error: {e}")))?;
    Ok(Json(models))
}

/// Parses an uploaded study file to text (format chosen by extension, like the original handler).
pub fn extract_text(file_name: &str, data: &[u8]) -> Result<String, String> {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e).unwrap_or("txt");
    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| e.to_string())?;
    temp_file.write_all(data).map_err(|e| e.to_string())?;
    let (_, temp_path) = temp_file.keep().map_err(|e| e.to_string())?;
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let parsed = parse_file(&temp_path_str).map_err(|e| format!("Parse error: {e}"));
    let _ = std::fs::remove_file(&temp_path_str);
    parsed
}

pub async fn generate_exam_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<GenerateResult>, (StatusCode, String)> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name = String::new();
    let mut params_json = String::new();
    let mut endpoint = String::new();
    let mut api_key = String::new();
    let mut model = String::new();
    let mut options_json = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().unwrap_or("unknown").to_string();
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
                        .to_vec(),
                );
            }
            "params" => {
                params_json = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            }
            "endpoint" => {
                endpoint = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            }
            "api_key" => {
                api_key = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            }
            "model" => {
                model = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            }
            "options" => {
                options_json = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
            }
            _ => {}
        }
    }

    let Resolved { endpoint, api_key, model, used_server_key, custom_endpoint } = resolve_ai(Some(&endpoint), Some(&api_key), Some(&model))?;
    state.legacy.check(&headers, custom_endpoint.then_some(endpoint.as_str()), used_server_key).await?;

    let params: ExamParams = serde_json::from_str(&params_json)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid params: {e}")))?;

    let text = match params.text.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            let file_data =
                file_data.ok_or((StatusCode::BAD_REQUEST, "No file uploaded".to_string()))?;
            extract_text(&file_name, &file_data).map_err(|e| (StatusCode::BAD_REQUEST, e))?
        }
    };

    let options: Option<AIRequestOptions> = if options_json.trim().is_empty() {
        None
    } else {
        serde_json::from_str(&options_json)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid options: {e}")))?
    };

    let client = AIClient::new(&endpoint, &api_key)
        .with_options(options)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid AI options: {e}")))?;
    let questions = generate_exam(&client, &text, &params, &model)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("AI error: {e}")))?;

    Ok(Json(GenerateResult { questions }))
}

pub async fn export_handler(
    Query(params): Query<ExportQuery>,
) -> Result<Response, (StatusCode, String)> {
    let questions: Vec<Question> = serde_json::from_str(&params.questions)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid questions JSON: {e}")))?;

    let mut buf = vec![];
    exameow_core::export::export_csv_to_writer(&questions, &mut buf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"questions.csv\"",
        )
        .body(axum::body::Body::from(buf))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub async fn export_xlsx_handler(
    Json(questions): Json<Vec<Question>>,
) -> Result<Response, (StatusCode, String)> {
    let data = exameow_core::export::export_xlsx_to_writer(&questions)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"exameow_questions.xlsx\"",
        )
        .body(axum::body::Body::from(data))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// The shared config store holds the operator's AI key, so only a caller on this machine/LAN may read or replace it.
/// An outside visitor of a public deployment gets nothing (they enter their own key, which stays in their browser).
fn require_local(headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let local = crate::relay::client_ip(headers).parse::<std::net::IpAddr>().map_or(false, crate::client_ip::is_local);
    if local { Ok(()) } else { Err(crate::relay::err(StatusCode::FORBIDDEN, "local_only")) }
}

pub async fn save_config_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(config): Json<AIConfigData>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_local(&headers)?;
    state
        .config_store
        .save(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Save error: {e}")))?;
    Ok(StatusCode::OK)
}

pub async fn load_config_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Option<AIConfigData>>, (StatusCode, String)> {
    // An outsider simply sees "nothing saved": the page still works, with their own key.
    if require_local(&headers).is_err() {
        return Ok(Json(None));
    }
    let config = state
        .config_store
        .load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Load error: {e}")))?;
    Ok(Json(config))
}

#[derive(Deserialize)]
pub struct AnswerRequest {
    pub question: String,
    pub language: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub options: Option<AIRequestOptions>,
}

pub async fn answer_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AnswerRequest>,
) -> Result<Json<AnswerResult>, (StatusCode, String)> {
    if req.question.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Question is empty".to_string()));
    }

    let Resolved { endpoint, api_key, model, used_server_key, custom_endpoint } =
        resolve_ai(req.endpoint.as_deref(), req.api_key.as_deref(), req.model.as_deref())?;
    state.legacy.check(&headers, custom_endpoint.then_some(endpoint.as_str()), used_server_key).await?;

    let language = req.language.filter(|s| !s.is_empty()).unwrap_or_else(|| "Chinese".to_string());

    let client = AIClient::new(&endpoint, &api_key)
        .with_options(req.options)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid AI options: {e}")))?;
    let result = answer_question(&client, &req.question, &language, &model)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("AI error: {e}")))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct JudgeRequest {
    pub stem: String,
    pub reference_answer: String,
    pub analysis: Option<String>,
    pub user_answer: String,
    pub language: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub options: Option<AIRequestOptions>,
}

pub async fn judge_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<JudgeRequest>,
) -> Result<Json<JudgeResult>, (StatusCode, String)> {
    if req.user_answer.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "User answer is empty".to_string()));
    }

    let Resolved { endpoint, api_key, model, used_server_key, custom_endpoint } =
        resolve_ai(req.endpoint.as_deref(), req.api_key.as_deref(), req.model.as_deref())?;
    state.legacy.check(&headers, custom_endpoint.then_some(endpoint.as_str()), used_server_key).await?;

    let language = req.language.filter(|s| !s.is_empty()).unwrap_or_else(|| "Chinese".to_string());
    let analysis = req.analysis.unwrap_or_default();

    let client = AIClient::new(&endpoint, &api_key)
        .with_options(req.options)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid AI options: {e}")))?;
    let result = judge_answer(
        &client,
        &req.stem,
        &req.reference_answer,
        &analysis,
        &req.user_answer,
        &language,
        &model,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("AI error: {e}")))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct ExplainRequest {
    pub stem: String,
    pub reference_answer: String,
    pub analysis: Option<String>,
    pub language: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub options: Option<AIRequestOptions>,
}

pub async fn explain_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ExplainRequest>,
) -> Result<Json<ExplainResult>, (StatusCode, String)> {
    if req.stem.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Question is empty".to_string()));
    }

    let Resolved { endpoint, api_key, model, used_server_key, custom_endpoint } =
        resolve_ai(req.endpoint.as_deref(), req.api_key.as_deref(), req.model.as_deref())?;
    state.legacy.check(&headers, custom_endpoint.then_some(endpoint.as_str()), used_server_key).await?;

    let language = req.language.filter(|s| !s.is_empty()).unwrap_or_else(|| "Chinese".to_string());
    let analysis = req.analysis.unwrap_or_default();

    let client = AIClient::new(&endpoint, &api_key)
        .with_options(req.options)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid AI options: {e}")))?;
    let result = explain_question(
        &client,
        &req.stem,
        &req.reference_answer,
        &analysis,
        &language,
        &model,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("AI error: {e}")))?;
    Ok(Json(result))
}

#[derive(Serialize)]
pub struct ServerConfigInfo {
    pub has_env_ai: bool,
    pub endpoint: String,
    pub model: String,
}

pub async fn server_config_info_handler(headers: HeaderMap) -> Json<ServerConfigInfo> {
    // the URL can name an internal host: only callers on this machine/LAN get it, everyone else just learns "AI is available"
    let endpoint = if require_local(&headers).is_ok() { ai_endpoint() } else { String::new() };
    let api_key = ai_api_key();
    let model = ai_model();
    Json(ServerConfigInfo {
        has_env_ai: !ai_endpoint().is_empty() && !api_key.is_empty(),
        endpoint,
        model,
    })
}
