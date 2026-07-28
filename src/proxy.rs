use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, warn};

use crate::error::{AppError, AppResult};
use crate::store::{ResolvedTarget, Store};

#[derive(Clone)]
pub struct ProxyState {
    pub http: Client,
    pub store: Store,
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
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        // common: https://api.openai.com/v1  or custom without /v1
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

/// Handle chat with ordered fallback. Stream fallback only before first byte committed.
pub async fn handle_chat_with_fallback(
    state: &ProxyState,
    public_model: &str,
    body: Bytes,
    stream: bool,
    targets: Vec<ResolvedTarget>,
) -> AppResult<Response> {
    let mut last_err: Option<UpstreamAttemptError> = None;

    for (idx, target) in targets.iter().enumerate() {
        let start = Instant::now();
        match forward_chat_completions(state, target, body.clone(), stream).await {
            Ok(resp) => {
                let latency = start.elapsed().as_millis() as i64;
                let _ = state.store.log_usage(
                    Some(public_model),
                    Some(&target.connection.id),
                    Some(200),
                    Some(latency),
                    None,
                );
                if stream {
                    return Ok(stream_response(resp).await?);
                } else {
                    return Ok(json_response(resp).await?);
                }
            }
            Err(e) => {
                warn!(
                    connection = %target.connection.name,
                    status = e.status,
                    retryable = e.retryable,
                    "upstream attempt failed"
                );
                let _ = state.store.log_usage(
                    Some(public_model),
                    Some(&target.connection.id),
                    Some(e.status as i64),
                    Some(start.elapsed().as_millis() as i64),
                    Some(&e.body.chars().take(500).collect::<String>()),
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

async fn json_response(resp: reqwest::Response) -> AppResult<Response> {
    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read upstream body: {e}")))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok((status, headers, bytes).into_response())
}

async fn stream_response(resp: reqwest::Response) -> AppResult<Response> {
    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::OK);
    let byte_stream = resp.bytes_stream().map(|chunk| {
        chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    });
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


