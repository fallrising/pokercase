use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use minijinja::context;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::server::AppState;
use crate::store::ConnectionPublic;
use crate::templates;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/admin", get(dashboard))
        .route("/admin/connections", get(connections_page))
        .route("/admin/connections/new", post(connections_create))
        .route(
            "/admin/connections/{id}/delete",
            post(connections_delete),
        )
        .route("/admin/routes", get(routes_page))
        .route("/admin/routes/new", post(routes_create))
        .route("/admin/routes/{id}/delete", post(routes_delete))
        .route("/admin/keys", get(keys_page))
        .route("/admin/keys/new", post(keys_create))
        .route("/admin/keys/{id}/delete", post(keys_delete))
        .route("/admin/usage", get(usage_page))
        .route("/admin/static/style.css", get(style_css))
}

fn page(name: &str, ctx: minijinja::Value) -> AppResult<Html<String>> {
    let body = templates::render(name, ctx).map_err(|e| AppError::Internal(e.into()))?;
    Ok(Html(body))
}

async fn dashboard(State(state): State<AppState>) -> AppResult<Html<String>> {
    let stats = state.store.stats()?;
    page(
        "dashboard",
        context! {
            title => "Dashboard",
            active => "dashboard",
            stats => stats,
            endpoint => format!("http://{}:{}/v1", state.cfg.host, state.cfg.port),
        },
    )
}

async fn connections_page(State(state): State<AppState>) -> AppResult<Html<String>> {
    let rows: Vec<ConnectionPublic> = state
        .store
        .list_connections()?
        .into_iter()
        .map(Into::into)
        .collect();
    page(
        "connections",
        context! {
            title => "Connections",
            active => "connections",
            connections => rows,
            flash => "",
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct ConnForm {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub priority: Option<i64>,
}

async fn connections_create(
    State(state): State<AppState>,
    Form(form): Form<ConnForm>,
) -> AppResult<Response> {
    let default_model = if form.default_model.trim().is_empty() {
        None
    } else {
        Some(form.default_model.trim().to_string())
    };
    state.store.upsert_connection(
        None,
        form.name.trim(),
        form.base_url.trim(),
        form.api_key.trim(),
        default_model.as_deref(),
        form.priority.unwrap_or(100),
        true,
    )?;
    Ok(Redirect::to("/admin/connections").into_response())
}

async fn connections_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> AppResult<Response> {
    state.store.delete_connection(&id)?;
    Ok(Redirect::to("/admin/connections").into_response())
}

async fn routes_page(State(state): State<AppState>) -> AppResult<Html<String>> {
    let routes = state.store.list_routes()?;
    let connections = state.store.list_connections()?;
    page(
        "routes",
        context! {
            title => "Routes",
            active => "routes",
            routes => routes,
            connections => connections,
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct RouteForm {
    pub public_model: String,
    pub connection_id: String,
    pub model_override: String,
}

async fn routes_create(
    State(state): State<AppState>,
    Form(form): Form<RouteForm>,
) -> AppResult<Response> {
    let model_override = if form.model_override.trim().is_empty() {
        None
    } else {
        Some(form.model_override.trim().to_string())
    };
    state.store.upsert_route(
        None,
        form.public_model.trim(),
        "fallback",
        &[(form.connection_id, model_override)],
    )?;
    Ok(Redirect::to("/admin/routes").into_response())
}

async fn routes_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> AppResult<Response> {
    state.store.delete_route(&id)?;
    Ok(Redirect::to("/admin/routes").into_response())
}

async fn keys_page(State(state): State<AppState>) -> AppResult<Html<String>> {
    let keys = state.store.list_api_keys()?;
    page(
        "keys",
        context! {
            title => "API Keys",
            active => "keys",
            keys => keys,
            new_secret => "",
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct KeyForm {
    pub name: String,
}

async fn keys_create(
    State(state): State<AppState>,
    Form(form): Form<KeyForm>,
) -> AppResult<Html<String>> {
    let name = if form.name.trim().is_empty() {
        "default"
    } else {
        form.name.trim()
    };
    let (row, secret) = state.store.create_api_key(name)?;
    let keys = state.store.list_api_keys()?;
    // show secret once
    let _ = row;
    page(
        "keys",
        context! {
            title => "API Keys",
            active => "keys",
            keys => keys,
            new_secret => secret,
        },
    )
}

async fn keys_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> AppResult<Response> {
    state.store.delete_api_key(&id)?;
    Ok(Redirect::to("/admin/keys").into_response())
}

async fn usage_page(State(state): State<AppState>) -> AppResult<Html<String>> {
    let events = state.store.recent_usage(50)?;
    page(
        "usage",
        context! {
            title => "Usage",
            active => "usage",
            events => events,
        },
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../templates/style.css"),
    )
}
