use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use minijinja::context;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::server::AppState;
use crate::store::ConnectionPublic;
use crate::templates;

pub const ADMIN_COOKIE: &str = "tr_admin";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/admin/login", get(login_page).post(login_submit))
        .route("/admin/logout", post(logout))
        .route("/admin", get(dashboard))
        .route("/admin/connections", get(connections_page))
        .route("/admin/connections/new", post(connections_create))
        .route("/admin/connections/{id}/edit", get(connections_edit_page))
        .route("/admin/connections/{id}/update", post(connections_update))
        .route(
            "/admin/connections/{id}/delete",
            post(connections_delete),
        )
        .route("/admin/routes", get(routes_page))
        .route("/admin/routes/new", post(routes_create))
        .route("/admin/routes/{id}/edit", get(routes_edit_page))
        .route("/admin/routes/{id}/update", post(routes_update))
        .route("/admin/routes/{id}/delete", post(routes_delete))
        .route("/admin/keys", get(keys_page))
        .route("/admin/keys/new", post(keys_create))
        .route("/admin/keys/{id}/delete", post(keys_delete))
        .route("/admin/usage", get(usage_page))
        .route("/admin/static/style.css", get(style_css))
}

/// True when admin token is unset, or header/cookie matches.
pub fn admin_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = &state.cfg.admin_token else {
        return true;
    };
    if let Some(v) = headers.get("x-admin-token").and_then(|v| v.to_str().ok()) {
        if v == token {
            return true;
        }
    }
    if let Some(cookie) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix(&format!("{ADMIN_COOKIE}=")) {
                if val == token {
                    return true;
                }
            }
        }
    }
    false
}

#[allow(clippy::result_large_err)]
fn require_admin_page(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    if admin_authorized(state, headers) {
        Ok(())
    } else {
        Err(Redirect::to("/admin/login").into_response())
    }
}

fn page(name: &str, ctx: minijinja::Value) -> AppResult<Html<String>> {
    let body = templates::render(name, ctx).map_err(|e| AppError::Internal(e.into()))?;
    Ok(Html(body))
}

async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LoginQuery>,
) -> Result<Html<String>, Response> {
    if state.cfg.admin_token.is_none() {
        return Err(Redirect::to("/admin").into_response());
    }
    if admin_authorized(&state, &headers) {
        return Err(Redirect::to("/admin").into_response());
    }
    page(
        "login",
        context! {
            title => "Admin login",
            error => q.error.unwrap_or_default(),
        },
    )
    .map_err(|e| e.into_response())
}

#[derive(Debug, Deserialize, Default)]
struct LoginQuery {
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    token: String,
}

async fn login_submit(
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> Response {
    let Some(expected) = &state.cfg.admin_token else {
        return Redirect::to("/admin").into_response();
    };
    if form.token.trim() == expected {
        let cookie = format!(
            "{ADMIN_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax",
            expected
        );
        (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, "/admin".to_string()),
                (header::SET_COOKIE, cookie),
            ],
        )
            .into_response()
    } else {
        Redirect::to("/admin/login?error=invalid").into_response()
    }
}

async fn logout() -> Response {
    let cookie = format!("{ADMIN_COOKIE}=; Path=/; HttpOnly; Max-Age=0");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/admin/login".to_string()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let stats = state.store.stats().map_err(|e| AppError::Internal(e).into_response())?;
    page(
        "dashboard",
        context! {
            title => "Dashboard",
            active => "dashboard",
            stats => stats,
            endpoint => format!("http://{}:{}/v1", state.cfg.host, state.cfg.port),
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

async fn connections_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let rows: Vec<ConnectionPublic> = state
        .store
        .list_connections()
        .map_err(|e| AppError::Internal(e).into_response())?
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
            edit => false,
            form => empty_conn_form(),
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

fn empty_conn_form() -> minijinja::Value {
    context! {
        id => "",
        name => "",
        base_url => "",
        default_model => "",
        priority => 100,
        enabled => true,
        input_price_per_m => "",
        output_price_per_m => "",
    }
}

#[derive(Debug, Deserialize)]
pub struct ConnForm {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub priority: Option<i64>,
    pub enabled: Option<String>,
    pub input_price_per_m: Option<String>,
    pub output_price_per_m: Option<String>,
}

fn parse_price(s: &Option<String>) -> Option<f64> {
    s.as_ref()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .and_then(|x| x.parse().ok())
}

async fn connections_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConnForm>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    let default_model = if form.default_model.trim().is_empty() {
        None
    } else {
        Some(form.default_model.trim().to_string())
    };
    let enabled = form.enabled.as_deref() != Some("0");
    state
        .store
        .upsert_connection(
            None,
            form.name.trim(),
            form.base_url.trim(),
            form.api_key.trim(),
            default_model.as_deref(),
            form.priority.unwrap_or(100),
            enabled,
            parse_price(&form.input_price_per_m),
            parse_price(&form.output_price_per_m),
        )
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/connections").into_response())
}

async fn connections_edit_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let c = state
        .store
        .get_connection(&id)
        .map_err(|e| AppError::Internal(e).into_response())?
        .ok_or_else(|| AppError::NotFound("connection not found".into()).into_response())?;
    let rows: Vec<ConnectionPublic> = state
        .store
        .list_connections()
        .map_err(|e| AppError::Internal(e).into_response())?
        .into_iter()
        .map(Into::into)
        .collect();
    page(
        "connections",
        context! {
            title => "Edit connection",
            active => "connections",
            connections => rows,
            flash => "",
            edit => true,
            form => context! {
                id => c.id,
                name => c.name,
                base_url => c.base_url,
                default_model => c.default_model.unwrap_or_default(),
                priority => c.priority,
                enabled => c.enabled,
                input_price_per_m => c.input_price_per_m.map(|p| p.to_string()).unwrap_or_default(),
                output_price_per_m => c.output_price_per_m.map(|p| p.to_string()).unwrap_or_default(),
            },
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

async fn connections_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<ConnForm>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    if state
        .store
        .get_connection(&id)
        .map_err(|e| AppError::Internal(e).into_response())?
        .is_none()
    {
        return Err(AppError::NotFound("connection not found".into()).into_response());
    }
    let default_model = if form.default_model.trim().is_empty() {
        None
    } else {
        Some(form.default_model.trim().to_string())
    };
    let enabled = form.enabled.as_deref() != Some("0");
    state
        .store
        .upsert_connection(
            Some(id),
            form.name.trim(),
            form.base_url.trim(),
            form.api_key.trim(), // empty keeps existing
            default_model.as_deref(),
            form.priority.unwrap_or(100),
            enabled,
            parse_price(&form.input_price_per_m),
            parse_price(&form.output_price_per_m),
        )
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/connections").into_response())
}

async fn connections_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    state
        .store
        .delete_connection(&id)
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/connections").into_response())
}

async fn routes_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let routes = state
        .store
        .list_routes()
        .map_err(|e| AppError::Internal(e).into_response())?;
    let connections = state
        .store
        .list_connections()
        .map_err(|e| AppError::Internal(e).into_response())?;
    page(
        "routes",
        context! {
            title => "Routes",
            active => "routes",
            routes => routes,
            connections => connections,
            edit => false,
            form => empty_route_form(),
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

fn empty_route_form() -> minijinja::Value {
    context! {
        id => "",
        public_model => "",
        strategy => "fallback",
        t0_conn => "",
        t0_model => "",
        t1_conn => "",
        t1_model => "",
        t2_conn => "",
        t2_model => "",
        t3_conn => "",
        t3_model => "",
        t4_conn => "",
        t4_model => "",
    }
}

#[derive(Debug, Deserialize)]
pub struct RouteForm {
    pub public_model: String,
    #[serde(default = "default_strategy_form")]
    pub strategy: String,
    pub t0_conn: Option<String>,
    pub t0_model: Option<String>,
    pub t1_conn: Option<String>,
    pub t1_model: Option<String>,
    pub t2_conn: Option<String>,
    pub t2_model: Option<String>,
    pub t3_conn: Option<String>,
    pub t3_model: Option<String>,
    pub t4_conn: Option<String>,
    pub t4_model: Option<String>,
}

fn default_strategy_form() -> String {
    "fallback".into()
}

fn collect_targets(form: &RouteForm) -> Vec<(String, Option<String>)> {
    let slots = [
        (&form.t0_conn, &form.t0_model),
        (&form.t1_conn, &form.t1_model),
        (&form.t2_conn, &form.t2_model),
        (&form.t3_conn, &form.t3_model),
        (&form.t4_conn, &form.t4_model),
    ];
    let mut out = Vec::new();
    for (conn, model) in slots {
        let Some(cid) = conn.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) else {
            continue;
        };
        let mo = model
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push((cid.to_string(), mo));
    }
    out
}

async fn routes_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RouteForm>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    let targets = collect_targets(&form);
    if targets.is_empty() {
        return Err(AppError::BadRequest("at least one target required".into()).into_response());
    }
    state
        .store
        .upsert_route(
            None,
            form.public_model.trim(),
            form.strategy.trim(),
            &targets,
        )
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/routes").into_response())
}

async fn routes_edit_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let route = state
        .store
        .get_route(&id)
        .map_err(|e| AppError::Internal(e).into_response())?
        .ok_or_else(|| AppError::NotFound("route not found".into()).into_response())?;
    let routes = state
        .store
        .list_routes()
        .map_err(|e| AppError::Internal(e).into_response())?;
    let connections = state
        .store
        .list_connections()
        .map_err(|e| AppError::Internal(e).into_response())?;

    let mut slots: Vec<(String, String)> = (0..5).map(|_| (String::new(), String::new())).collect();
    for (i, t) in route.targets.iter().take(5).enumerate() {
        slots[i] = (
            t.connection_id.clone(),
            t.model_override.clone().unwrap_or_default(),
        );
    }

    page(
        "routes",
        context! {
            title => "Edit route",
            active => "routes",
            routes => routes,
            connections => connections,
            edit => true,
            form => context! {
                id => route.id,
                public_model => route.public_model,
                strategy => route.strategy,
                t0_conn => slots[0].0.clone(),
                t0_model => slots[0].1.clone(),
                t1_conn => slots[1].0.clone(),
                t1_model => slots[1].1.clone(),
                t2_conn => slots[2].0.clone(),
                t2_model => slots[2].1.clone(),
                t3_conn => slots[3].0.clone(),
                t3_model => slots[3].1.clone(),
                t4_conn => slots[4].0.clone(),
                t4_model => slots[4].1.clone(),
            },
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

async fn routes_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<RouteForm>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    if state
        .store
        .get_route(&id)
        .map_err(|e| AppError::Internal(e).into_response())?
        .is_none()
    {
        return Err(AppError::NotFound("route not found".into()).into_response());
    }
    let targets = collect_targets(&form);
    if targets.is_empty() {
        return Err(AppError::BadRequest("at least one target required".into()).into_response());
    }
    state
        .store
        .upsert_route(
            Some(id),
            form.public_model.trim(),
            form.strategy.trim(),
            &targets,
        )
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/routes").into_response())
}

async fn routes_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    state
        .store
        .delete_route(&id)
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/routes").into_response())
}

async fn keys_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let keys = state
        .store
        .list_api_keys()
        .map_err(|e| AppError::Internal(e).into_response())?;
    page(
        "keys",
        context! {
            title => "API Keys",
            active => "keys",
            keys => keys,
            new_secret => "",
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

#[derive(Debug, Deserialize)]
pub struct KeyForm {
    pub name: String,
}

async fn keys_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<KeyForm>,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let name = if form.name.trim().is_empty() {
        "default"
    } else {
        form.name.trim()
    };
    let (_row, secret) = state
        .store
        .create_api_key(name)
        .map_err(|e| AppError::Internal(e).into_response())?;
    let keys = state
        .store
        .list_api_keys()
        .map_err(|e| AppError::Internal(e).into_response())?;
    page(
        "keys",
        context! {
            title => "API Keys",
            active => "keys",
            keys => keys,
            new_secret => secret,
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

async fn keys_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin_page(&state, &headers)?;
    state
        .store
        .delete_api_key(&id)
        .map_err(|e| AppError::Internal(e).into_response())?;
    Ok(Redirect::to("/admin/keys").into_response())
}

async fn usage_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    require_admin_page(&state, &headers)?;
    let events = state
        .store
        .recent_usage(50)
        .map_err(|e| AppError::Internal(e).into_response())?;
    let cost_total = state
        .store
        .usage_cost_total()
        .map_err(|e| AppError::Internal(e).into_response())?;
    page(
        "usage",
        context! {
            title => "Usage",
            active => "usage",
            events => events,
            cost_total => format!("{cost_total:.6}"),
            admin_protected => state.cfg.admin_token.is_some(),
        },
    )
    .map_err(|e| e.into_response())
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../templates/style.css"),
    )
}
