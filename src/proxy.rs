use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::claude::{self, StreamTranslateState};
use crate::cooldown::{cooldown_secs_for_status, CooldownMap};
use crate::error::{AppError, AppResult};
use crate::store::{ResolvedTarget, Store};

#[derive(Clone)]
pub struct ProxyState {
    pub http: Client,
    pub store: Store,
    pub cooldown: CooldownMap,
    /// Global counter for round-robin start offsets.
    pub rr_counter: Arc<AtomicU64>,
    pub sse_stall_secs: u64,
}

#[derive(Debug)]
pub struct UpstreamAttemptError {
    pub status: u16,
    pub body: String,
    pub retryable: bool,
}

pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 401 | 402 | 403 | 408 | 429) || (500..600).contains(&status)
}

/// Rewrite JSON body: set `model` to upstream model name.
pub fn rewrite_model(body: &Bytes, upstream_model: &str) -> AppResult<Bytes> {
    let mut v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| AppError::BadRequest("body must be a JSON object".into()))?;
    obj.insert("model".into(), Value::String(upstream_model.to_string()));
    Ok(Bytes::from(serde_json::to_vec(&v)?))
}

pub fn extract_model_and_stream(body: &Bytes) -> AppResult<(String, bool)> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let stream = v
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    if model.is_empty() {
        return Err(AppError::BadRequest("model is required".into()));
    }
    Ok((model, stream))
}

fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

pub async fn forward_chat_completions(
    state: &ProxyState,
    target: &ResolvedTarget,
    body: Bytes,
    stream: bool,
) -> Result<reqwest::Response, UpstreamAttemptError> {
    let url = chat_completions_url(&target.connection.base_url);
    let rewritten = match rewrite_model(&body, &target.upstream_model) {
        Ok(b) => b,
        Err(e) => {
            return Err(UpstreamAttemptError {
                status: 400,
                body: e.to_string(),
                retryable: false,
            });
        }
    };

    debug!(
        url = %url,
        connection = %target.connection.name,
        model = %target.upstream_model,
        stream,
        "forwarding chat completions"
    );

    let mut req = state
        .http
        .post(&url)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", target.connection.api_key),
        )
        .body(rewritten);

    if stream {
        req = req.header("accept", "text/event-stream");
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if status == 200 {
                Ok(resp)
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(UpstreamAttemptError {
                    status,
                    body,
                    retryable: is_retryable_status(status),
                })
            }
        }
        Err(e) => Err(UpstreamAttemptError {
            status: 502,
            body: format!("upstream request failed: {e}"),
            retryable: true,
        }),
    }
}

/// Order targets: skip cooled; apply round-robin rotation when strategy is round_robin.
pub fn order_targets(
    targets: Vec<ResolvedTarget>,
    strategy: &str,
    cooldown: &CooldownMap,
    rr_counter: &AtomicU64,
) -> Vec<ResolvedTarget> {
    let mut active: Vec<ResolvedTarget> = Vec::new();
    let mut cooled: Vec<ResolvedTarget> = Vec::new();
    for t in targets {
        if cooldown.is_cooled(&t.connection.id) {
            cooled.push(t);
        } else {
            active.push(t);
        }
    }
    // Prefer non-cooled; if all cooled, still try them (last resort).
    let mut ordered = if active.is_empty() { cooled } else { active };

    if strategy.eq_ignore_ascii_case("round_robin") && ordered.len() > 1 {
        let n = ordered.len() as u64;
        let start = (rr_counter.fetch_add(1, Ordering::Relaxed) % n) as usize;
        ordered.rotate_left(start);
    }
    ordered
}

/// Handle chat with ordered fallback. Stream fallback only before first byte committed.
pub async fn handle_chat_with_fallback(
    state: &ProxyState,
    public_model: &str,
    body: Bytes,
    stream: bool,
    targets: Vec<ResolvedTarget>,
    strategy: &str,
) -> AppResult<Response> {
    let targets = order_targets(targets, strategy, &state.cooldown, &state.rr_counter);
    let mut last_err: Option<UpstreamAttemptError> = None;

    for (idx, target) in targets.iter().enumerate() {
        let start = Instant::now();
        match forward_chat_completions(state, target, body.clone(), stream).await {
            Ok(resp) => {
                let latency = start.elapsed().as_millis() as i64;
                state.cooldown.clear(&target.connection.id);
                if stream {
                    let _ = state.store.log_usage(
                        Some(public_model),
                        Some(&target.connection.id),
                        Some(200),
                        Some(latency),
                        None,
                        None,
                        None,
                        None,
                    );
                    return stream_response(resp, state.sse_stall_secs, None).await;
                } else {
                    let bytes = resp
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(anyhow::anyhow!("read upstream body: {e}")))?;
                    let (pt, ct) = extract_usage_tokens(&bytes);
                    let cost = estimate_cost(
                        pt,
                        ct,
                        target.connection.input_price_per_m,
                        target.connection.output_price_per_m,
                    );
                    let _ = state.store.log_usage(
                        Some(public_model),
                        Some(&target.connection.id),
                        Some(200),
                        Some(latency),
                        None,
                        pt,
                        ct,
                        cost,
                    );
                    return Ok(json_bytes_response(StatusCode::OK, bytes));
                }
            }
            Err(e) => {
                warn!(
                    connection = %target.connection.name,
                    status = e.status,
                    retryable = e.retryable,
                    "upstream attempt failed"
                );
                let secs = cooldown_secs_for_status(e.status);
                if secs > 0 {
                    state.cooldown.mark(&target.connection.id, secs);
                }
                let _ = state.store.log_usage(
                    Some(public_model),
                    Some(&target.connection.id),
                    Some(e.status as i64),
                    Some(start.elapsed().as_millis() as i64),
                    Some(&e.body.chars().take(500).collect::<String>()),
                    None,
                    None,
                    None,
                );
                if e.retryable && idx + 1 < targets.len() {
                    last_err = Some(e);
                    continue;
                }
                return Err(AppError::Upstream {
                    status: e.status,
                    body: e.body,
                });
            }
        }
    }

    let e = last_err.unwrap_or(UpstreamAttemptError {
        status: 503,
        body: "no upstream targets".into(),
        retryable: false,
    });
    Err(AppError::Upstream {
        status: e.status,
        body: e.body,
    })
}

/// Anthropic-native path: translate request → OpenAI forward → translate response.
pub async fn handle_anthropic_messages(
    state: &ProxyState,
    public_model: &str,
    body: Bytes,
    stream: bool,
    targets: Vec<ResolvedTarget>,
    strategy: &str,
) -> AppResult<Response> {
    let oai_body = claude::anthropic_to_openai(&body)?;
    let targets = order_targets(targets, strategy, &state.cooldown, &state.rr_counter);
    let mut last_err: Option<UpstreamAttemptError> = None;

    for (idx, target) in targets.iter().enumerate() {
        let start = Instant::now();
        match forward_chat_completions(state, target, oai_body.clone(), stream).await {
            Ok(resp) => {
                let latency = start.elapsed().as_millis() as i64;
                state.cooldown.clear(&target.connection.id);
                if stream {
                    let _ = state.store.log_usage(
                        Some(public_model),
                        Some(&target.connection.id),
                        Some(200),
                        Some(latency),
                        None,
                        None,
                        None,
                        None,
                    );
                    return stream_response(
                        resp,
                        state.sse_stall_secs,
                        Some(public_model.to_string()),
                    )
                    .await;
                } else {
                    let bytes = resp
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(anyhow::anyhow!("read upstream body: {e}")))?;
                    let (pt, ct) = extract_usage_tokens(&bytes);
                    let cost = estimate_cost(
                        pt,
                        ct,
                        target.connection.input_price_per_m,
                        target.connection.output_price_per_m,
                    );
                    let _ = state.store.log_usage(
                        Some(public_model),
                        Some(&target.connection.id),
                        Some(200),
                        Some(latency),
                        None,
                        pt,
                        ct,
                        cost,
                    );
                    let anth = claude::openai_to_anthropic(&bytes, public_model)?;
                    return Ok(json_bytes_response(StatusCode::OK, anth));
                }
            }
            Err(e) => {
                let secs = cooldown_secs_for_status(e.status);
                if secs > 0 {
                    state.cooldown.mark(&target.connection.id, secs);
                }
                let _ = state.store.log_usage(
                    Some(public_model),
                    Some(&target.connection.id),
                    Some(e.status as i64),
                    Some(start.elapsed().as_millis() as i64),
                    Some(&e.body.chars().take(500).collect::<String>()),
                    None,
                    None,
                    None,
                );
                if e.retryable && idx + 1 < targets.len() {
                    last_err = Some(e);
                    continue;
                }
                return Err(AppError::Upstream {
                    status: e.status,
                    body: e.body,
                });
            }
        }
    }

    let e = last_err.unwrap_or(UpstreamAttemptError {
        status: 503,
        body: "no upstream targets".into(),
        retryable: false,
    });
    Err(AppError::Upstream {
        status: e.status,
        body: e.body,
    })
}

fn extract_usage_tokens(bytes: &Bytes) -> (Option<i64>, Option<i64>) {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return (None, None);
    };
    let pt = v.pointer("/usage/prompt_tokens").and_then(|n| n.as_i64());
    let ct = v
        .pointer("/usage/completion_tokens")
        .and_then(|n| n.as_i64());
    (pt, ct)
}

pub fn estimate_cost(
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    input_price_per_m: Option<f64>,
    output_price_per_m: Option<f64>,
) -> Option<f64> {
    let pt = prompt_tokens? as f64;
    let ct = completion_tokens.unwrap_or(0) as f64;
    let ip = input_price_per_m.unwrap_or(0.0);
    let op = output_price_per_m.unwrap_or(0.0);
    if ip == 0.0 && op == 0.0 {
        return None;
    }
    Some((pt * ip + ct * op) / 1_000_000.0)
}

fn json_bytes_response(status: StatusCode, bytes: Bytes) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (status, headers, bytes).into_response()
}

/// Stream upstream SSE with stall timeout. If `anthropic_model` is set, translate chunks.
async fn stream_response(
    resp: reqwest::Response,
    stall_secs: u64,
    anthropic_model: Option<String>,
) -> AppResult<Response> {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let stall = Duration::from_secs(stall_secs.max(5));
    let mut upstream = resp.bytes_stream();

    let byte_stream = async_stream::stream! {
        if let Some(model) = anthropic_model {
            let mut state = StreamTranslateState::new(model);
            let mut buf = String::new();
            loop {
                match timeout(stall, upstream.next()).await {
                    Ok(Some(Ok(chunk))) => {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(pos) = buf.find('\n') {
                            let mut line = buf[..pos].to_string();
                            buf = buf[pos + 1..].to_string();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                            if let Some(data) = line.strip_prefix("data:").or_else(|| line.strip_prefix("data: ")) {
                                let data = data.trim_start();
                                // handle "data: xxx" with space after colon already trimmed
                                let data = data.strip_prefix(' ').unwrap_or(data);
                                for ev in claude::openai_sse_chunk_to_anthropic(data, &mut state) {
                                    let out = format!("{ev}\n\n");
                                    yield Ok::<Bytes, std::io::Error>(Bytes::from(out));
                                }
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        yield Err(std::io::Error::other(e.to_string()));
                        break;
                    }
                    Ok(None) => {
                        if !state.message_stop_sent {
                            for ev in claude::openai_sse_chunk_to_anthropic("[DONE]", &mut state) {
                                let out = format!("{ev}\n\n");
                                yield Ok(Bytes::from(out));
                            }
                        }
                        break;
                    }
                    Err(_) => {
                        warn!("SSE stall timeout");
                        yield Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("SSE stall: no data for {stall_secs}s"),
                        ));
                        break;
                    }
                }
            }
        } else {
            loop {
                match timeout(stall, upstream.next()).await {
                    Ok(Some(Ok(chunk))) => {
                        yield Ok::<Bytes, std::io::Error>(chunk);
                    }
                    Ok(Some(Err(e))) => {
                        yield Err(std::io::Error::other(e.to_string()));
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        warn!("SSE stall timeout");
                        yield Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("SSE stall: no data for {stall_secs}s"),
                        ));
                        break;
                    }
                }
            }
        }
    };

    let body = Body::from_stream(byte_stream);
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        axum::http::header::CONNECTION,
        HeaderValue::from_static("keep-alive"),
    );
    Ok((status, headers, body).into_response())
}
