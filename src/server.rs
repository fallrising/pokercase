use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::admin;
use crate::cancel::CancelOnDrop;
use crate::claude;
use crate::config::AppConfig;
use crate::cooldown::CooldownMap;
use crate::error::{AppError, AppResult};
use crate::http_client;
use crate::proxy::{self, ProxyState};
use crate::resolve;
use crate::responses;
use crate::store::Store;
use crate::web;

#[derive(Clone)]
pub struct AppState {
    pub cfg: AppConfig,
    pub store: Store,
    pub proxy: ProxyState,
}

pub fn build_app(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/messages", post(anthropic_messages))
        .route("/responses", post(openai_responses))
        .route("/models", get(list_models))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    Router::new()
        .route("/health", get(health))
        .nest("/v1", v1)
        .route("/chat/completions", post(chat_completions))
        .route("/messages", post(anthropic_messages))
        .route("/responses", post(openai_responses))
        .route("/models", get(list_models))
        .merge(admin::router())
        .merge(web::router())
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(cfg: AppConfig) -> Result<()> {
    let store = Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;
    let http = http_client::build_http_client()?;

    let state = AppState {
        cfg: cfg.clone(),
        store: store.clone(),
        proxy: ProxyState {
            http,
            store: store.clone(),
            cooldown: CooldownMap::new(),
            rr_counter: Arc::new(AtomicU64::new(0)),
            sse_stall_secs: cfg.sse_stall_secs,
            token_saver: cfg.token_saver,
            token_saver_max_chars: cfg.token_saver_max_chars,
        },
    };

    let app = build_app(state);

    let addr: SocketAddr = cfg
        .listen_addr()
        .parse()
        .with_context(|| format!("invalid listen addr {}", cfg.listen_addr()))?;
    info!(%addr, data_dir = %cfg.data_dir.display(), "thinrouter listening");
    info!("  proxy     : http://{addr}/v1/chat/completions");
    info!("  claude    : http://{addr}/v1/messages");
    info!("  responses : http://{addr}/v1/responses");
    info!("  admin     : http://{addr}/admin");
    if cfg.token_saver {
        info!(
            max_chars = cfg.token_saver_max_chars,
            "token-saver enabled"
        );
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok", "service": "thinrouter"}))
}

async fn require_api_key(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let key = extract_api_key(request.headers());
    match key {
        Some(k) if state.store.verify_api_key(&k)? => Ok(next.run(request).await),
        Some(_) => Err(AppError::Unauthorized),
        None => {
            if state.store.verify_api_key("")? {
                Ok(next.run(request).await)
            } else {
                Err(AppError::Unauthorized)
            }
        }
    }
}

fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = v.to_str() {
            let s = s.trim();
            if let Some(rest) = s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")) {
                return Some(rest.trim().to_string());
            }
        }
    }
    if let Some(v) = headers.get("x-api-key") {
        if let Ok(s) = v.to_str() {
            return Some(s.trim().to_string());
        }
    }
    None
}

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    ensure_api_key(&state, &headers)?;
    let (guard, cancel) = CancelOnDrop::new();

    let (public_model, stream) = proxy::extract_model_and_stream(&body)?;
    let resolved = resolve::resolve_targets(&state.store, &public_model)?;
    proxy::handle_chat_with_fallback(
        &state.proxy,
        &public_model,
        body,
        stream,
        resolved.targets,
        &resolved.strategy,
        cancel,
        guard,
    )
    .await
}

async fn anthropic_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    ensure_api_key(&state, &headers)?;
    let (guard, cancel) = CancelOnDrop::new();

    let (public_model, stream) = claude::extract_anthropic_model_stream(&body)?;
    let resolved = resolve::resolve_targets(&state.store, &public_model)?;
    proxy::handle_anthropic_messages(
        &state.proxy,
        &public_model,
        body,
        stream,
        resolved.targets,
        &resolved.strategy,
        cancel,
        guard,
    )
    .await
}

async fn openai_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    ensure_api_key(&state, &headers)?;
    let (guard, cancel) = CancelOnDrop::new();

    let (public_model, stream) = responses::extract_responses_model_stream(&body)?;
    let resolved = resolve::resolve_targets(&state.store, &public_model)?;
    proxy::handle_responses(
        &state.proxy,
        &public_model,
        body,
        stream,
        resolved.targets,
        &resolved.strategy,
        cancel,
        guard,
    )
    .await
}

fn ensure_api_key(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    let key = extract_api_key(headers);
    match key {
        Some(k) if state.store.verify_api_key(&k)? => Ok(()),
        Some(_) => Err(AppError::Unauthorized),
        None => {
            if state.store.verify_api_key("")? {
                Ok(())
            } else {
                Err(AppError::Unauthorized)
            }
        }
    }
}

async fn list_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    ensure_api_key(&state, &headers)?;
    let mut data = Vec::new();
    for route in state.store.list_routes()? {
        data.push(json!({
            "id": route.public_model,
            "object": "model",
            "owned_by": "thinrouter",
            "kind": "route",
        }));
    }
    for conn in state.store.list_connections()? {
        if !conn.enabled {
            continue;
        }
        if let Some(m) = &conn.default_model {
            data.push(json!({
                "id": format!("{}/{}", conn.name, m),
                "object": "model",
                "owned_by": conn.name,
                "kind": "connection",
            }));
        }
        data.push(json!({
            "id": conn.name,
            "object": "model",
            "owned_by": conn.name,
            "kind": "connection_default",
        }));
    }
    Ok(Json(json!({
        "object": "list",
        "data": data,
    })))
}

#[allow(dead_code)]
fn _status_ok() -> StatusCode {
    StatusCode::OK
}
