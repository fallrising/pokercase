use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::server::AppState;
use crate::store::ConnectionPublic;
use crate::web::{admin_authorized, ADMIN_COOKIE};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/stats", get(stats))
        .route(
            "/admin/api/connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/admin/api/connections/{id}",
            get(get_connection)
                .put(update_connection)
                .delete(delete_connection),
        )
        .route(
            "/admin/api/connections/{id}/test",
            post(test_connection),
        )
        .route("/admin/api/routes", get(list_routes).post(create_route))
        .route(
            "/admin/api/routes/{id}",
            get(get_route).put(update_route).delete(delete_route),
        )
        .route("/admin/api/keys", get(list_keys).post(create_key))
        .route("/admin/api/keys/{id}", delete(delete_key))
        .route("/admin/api/keys/{id}/enable", post(enable_key))
        .route("/admin/api/keys/{id}/disable", post(disable_key))
        .route("/admin/api/usage", get(usage))
        .route("/admin/api/usage/daily", get(usage_daily))
        .route("/admin/api/usage/export.csv", get(usage_export_csv))
        .route(
            "/admin/api/connections/oauth/import",
            post(import_oauth_connection),
        )
}

fn check_admin(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    if admin_authorized(state, headers) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    Ok(Json(state.store.stats()?))
}

async fn list_connections(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    let rows: Vec<ConnectionPublic> = state
        .store
        .list_connections()?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(json!({ "data": rows })))
}

#[derive(Debug, Deserialize)]
pub struct ConnectionInput {
    #[allow(dead_code)]
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub default_model: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub input_price_per_m: Option<f64>,
    pub output_price_per_m: Option<f64>,
}

fn default_priority() -> i64 {
    100
}
fn default_true() -> bool {
    true
}

async fn create_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ConnectionInput>,
) -> AppResult<(StatusCode, Json<Value>)> {
    check_admin(&state, &headers)?;
    if input.name.trim().is_empty() || input.base_url.trim().is_empty() {
        return Err(AppError::BadRequest(
            "name and base_url are required".into(),
        ));
    }
    if input.api_key.is_empty() {
        return Err(AppError::BadRequest("api_key is required".into()));
    }
    let row = state.store.upsert_connection(
        None,
        input.name.trim(),
        input.base_url.trim(),
        &input.api_key,
        input.default_model.as_deref(),
        input.priority,
        input.enabled,
        input.input_price_per_m,
        input.output_price_per_m,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(json!(ConnectionPublic::from(row))),
    ))
}

async fn get_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    let row = state
        .store
        .get_connection(&id)?
        .ok_or_else(|| AppError::NotFound("connection not found".into()))?;
    Ok(Json(json!(ConnectionPublic::from(row))))
}

async fn update_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ConnectionInput>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    let existing = state
        .store
        .get_connection(&id)?
        .ok_or_else(|| AppError::NotFound("connection not found".into()))?;
    let api_key = if input.api_key.is_empty() {
        // empty means keep existing (store handles '' as keep when encrypted write)
        String::new()
    } else {
        input.api_key
    };
    let _ = existing;
    let row = state.store.upsert_connection(
        Some(id),
        input.name.trim(),
        input.base_url.trim(),
        &api_key,
        input.default_model.as_deref(),
        input.priority,
        input.enabled,
        input.input_price_per_m,
        input.output_price_per_m,
    )?;
    Ok(Json(json!(ConnectionPublic::from(row))))
}

async fn delete_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if !state.store.delete_connection(&id)? {
        return Err(AppError::NotFound("connection not found".into()));
    }
    Ok(Json(json!({"ok": true})))
}

async fn test_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    let conn = state
        .store
        .get_connection(&id)?
        .ok_or_else(|| AppError::NotFound("connection not found".into()))?;

    let base = conn.base_url.trim_end_matches('/');
    let url = if base.ends_with("/models") {
        base.to_string()
    } else {
        format!("{base}/models")
    };

    let resp = state
        .proxy
        .http
        .get(&url)
        .header("authorization", format!("Bearer {}", conn.bearer_token()))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("test request failed: {e}")))?;

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let snippet: String = body.chars().take(300).collect();
    Ok(Json(json!({
        "ok": status == 200,
        "status": status,
        "url": url,
        "body_snippet": snippet,
    })))
}

#[derive(Debug, Deserialize)]
pub struct RouteTargetInput {
    pub connection_id: String,
    pub model_override: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RouteInput {
    pub public_model: String,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    pub targets: Vec<RouteTargetInput>,
}

fn default_strategy() -> String {
    "fallback".into()
}

async fn list_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    Ok(Json(json!({ "data": state.store.list_routes()? })))
}

async fn get_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    let row = state
        .store
        .get_route(&id)?
        .ok_or_else(|| AppError::NotFound("route not found".into()))?;
    Ok(Json(json!(row)))
}

async fn create_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<RouteInput>,
) -> AppResult<(StatusCode, Json<Value>)> {
    check_admin(&state, &headers)?;
    if input.public_model.trim().is_empty() {
        return Err(AppError::BadRequest("public_model is required".into()));
    }
    if input.targets.is_empty() {
        return Err(AppError::BadRequest("at least one target required".into()));
    }
    let targets: Vec<(String, Option<String>)> = input
        .targets
        .into_iter()
        .map(|t| (t.connection_id, t.model_override))
        .collect();
    let row = state.store.upsert_route(
        None,
        input.public_model.trim(),
        &input.strategy,
        &targets,
    )?;
    Ok((StatusCode::CREATED, Json(json!(row))))
}

async fn update_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RouteInput>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if state.store.get_route(&id)?.is_none() {
        return Err(AppError::NotFound("route not found".into()));
    }
    if input.targets.is_empty() {
        return Err(AppError::BadRequest("at least one target required".into()));
    }
    let targets: Vec<(String, Option<String>)> = input
        .targets
        .into_iter()
        .map(|t| (t.connection_id, t.model_override))
        .collect();
    let row = state.store.upsert_route(
        Some(id),
        input.public_model.trim(),
        &input.strategy,
        &targets,
    )?;
    Ok(Json(json!(row)))
}

async fn delete_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if !state.store.delete_route(&id)? {
        return Err(AppError::NotFound("route not found".into()));
    }
    Ok(Json(json!({"ok": true})))
}

async fn list_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    Ok(Json(json!({ "data": state.store.list_api_keys()? })))
}

#[derive(Debug, Deserialize)]
pub struct KeyInput {
    pub name: String,
}

async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<KeyInput>,
) -> AppResult<(StatusCode, Json<Value>)> {
    check_admin(&state, &headers)?;
    let name = if input.name.trim().is_empty() {
        "default"
    } else {
        input.name.trim()
    };
    let (row, raw) = state.store.create_api_key(name)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": row,
            "secret": raw,
            "warning": "Store this secret now; it will not be shown again."
        })),
    ))
}

async fn delete_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if !state.store.delete_api_key(&id)? {
        return Err(AppError::NotFound("key not found".into()));
    }
    Ok(Json(json!({"ok": true})))
}

async fn enable_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if !state.store.set_api_key_enabled(&id, true)? {
        return Err(AppError::NotFound("key not found".into()));
    }
    Ok(Json(json!({"ok": true})))
}

async fn disable_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    if !state.store.set_api_key_enabled(&id, false)? {
        return Err(AppError::NotFound("key not found".into()));
    }
    Ok(Json(json!({"ok": true})))
}

async fn usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    Ok(Json(json!({ "data": state.store.recent_usage(100)? })))
}

async fn usage_daily(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    check_admin(&state, &headers)?;
    Ok(Json(json!({ "data": state.store.usage_by_day(30)? })))
}

async fn usage_export_csv(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    check_admin(&state, &headers)?;
    let csv = state.store.usage_csv(1000)?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/csv; charset=utf-8",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"thinrouter-usage.csv\"",
            ),
        ],
        csv,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct OAuthImportInput {
    pub name: String,
    /// Provider id: codex | claude | github_copilot | cursor | kiro | generic
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub meta: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_base_for_provider(provider: &str) -> &'static str {
    match provider {
        "codex" | "openai" => "https://api.openai.com/v1",
        "claude" | "anthropic" => "https://api.anthropic.com/v1",
        "github_copilot" | "copilot" => "https://api.githubcopilot.com",
        "cursor" => "https://api2.cursor.sh",
        "kiro" => "https://kiro.dev/api",
        _ => "https://api.openai.com/v1",
    }
}

async fn import_oauth_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<OAuthImportInput>,
) -> AppResult<(StatusCode, Json<Value>)> {
    check_admin(&state, &headers)?;
    if input.name.trim().is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    if input.access_token.trim().is_empty() {
        return Err(AppError::BadRequest("access_token is required".into()));
    }
    let provider = input.provider.trim();
    if provider.is_empty() {
        return Err(AppError::BadRequest("provider is required".into()));
    }
    let base = input
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_base_for_provider(provider));

    let row = state
        .store
        .upsert_oauth_connection(
            None,
            input.name.trim(),
            base,
            provider,
            input.access_token.trim(),
            input.refresh_token.as_deref(),
            input.expires_at.as_deref(),
            input.meta.as_deref(),
            input.default_model.as_deref(),
            input.priority,
            input.enabled,
        )
        .map_err(|e| AppError::Internal(e))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "connection": ConnectionPublic::from(row),
            "note": "OAuth/session tokens stored as oauth_import. Full browser OAuth per-provider is next; import unblocks personal subscriptions now. Upstream may still need provider-specific request shapes."
        })),
    ))
}

// re-export for cookie name used by tests/docs
#[allow(dead_code)]
pub fn admin_cookie_name() -> &'static str {
    ADMIN_COOKIE
}
