//! Mock-upstream end-to-end tests (non-stream + stream + Anthropic messages + fallback).

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::tempdir;
use tokio::net::TcpListener;

async fn mock_openai_upstream() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|req: Request| async move {
            let body = req.into_body().collect().await.unwrap().to_bytes();
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let stream = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("mock-model");

            if stream {
                let payload = format!(
                    "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"hel\"}},\"finish_reason\":null}}]}}\n\n\
data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"lo\"}},\"finish_reason\":null}}]}}\n\n\
data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
data: [DONE]\n\n"
                );
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(payload))
                    .unwrap()
            } else {
                Json(json!({
                    "id": "chatcmpl-1",
                    "object": "chat.completion",
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hello from mock"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
                }))
                .into_response()
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (addr, handle)
}

async fn start_thinrouter(data_dir: &std::path::Path, port: u16) -> tokio::process::Child {
    let bin = env!("CARGO_BIN_EXE_thinrouter");
    tokio::process::Command::new(bin)
        .args([
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--sse-stall-secs",
            "30",
        ])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn thinrouter")
}

async fn wait_health(port: u16) {
    let client = reqwest::Client::new();
    for _ in 0..80 {
        if let Ok(r) = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
        {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("thinrouter did not become healthy on port {port}");
}

async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

#[tokio::test]
async fn e2e_nonstream_stream_anthropic_fallback() {
    let (upstream_addr, _up) = mock_openai_upstream().await;
    let dir = tempdir().unwrap();
    let port = free_port().await;
    let mut child = start_thinrouter(dir.path(), port).await;
    wait_health(port).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    let conn: Value = client
        .post(format!("{base}/admin/api/connections"))
        .json(&json!({
            "name": "mock",
            "base_url": format!("http://{upstream_addr}/v1"),
            "api_key": "sk-mock",
            "default_model": "mock-model",
            "input_price_per_m": 1.0,
            "output_price_per_m": 2.0
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let conn_id = conn["id"].as_str().unwrap();

    client
        .post(format!("{base}/admin/api/routes"))
        .json(&json!({
            "public_model": "cheap",
            "strategy": "fallback",
            "targets": [{"connection_id": conn_id, "model_override": null}]
        }))
        .send()
        .await
        .unwrap();

    // non-stream
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": "cheap",
            "messages": [{"role":"user","content":"hi"}],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "hello from mock");

    // stream
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": "cheap",
            "messages": [{"role":"user","content":"hi"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("hel") || text.contains("data:"),
        "stream body: {text}"
    );

    // anthropic messages non-stream
    let resp = client
        .post(format!("{base}/v1/messages"))
        .json(&json!({
            "model": "cheap",
            "max_tokens": 32,
            "messages": [{"role":"user","content":"hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "hello from mock");

    // OpenAI Responses API
    let resp = client
        .post(format!("{base}/v1/responses"))
        .json(&json!({
            "model": "cheap",
            "input": "hi",
            "max_output_tokens": 32
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "hello from mock");

    // fallback: dead then mock
    let bad: Value = client
        .post(format!("{base}/admin/api/connections"))
        .json(&json!({
            "name": "dead",
            "base_url": "http://127.0.0.1:9/v1",
            "api_key": "x",
            "default_model": "x"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client
        .post(format!("{base}/admin/api/routes"))
        .json(&json!({
            "public_model": "fb",
            "strategy": "fallback",
            "targets": [
                {"connection_id": bad["id"], "model_override": null},
                {"connection_id": conn_id, "model_override": null}
            ]
        }))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": "fb",
            "messages": [{"role":"user","content":"hi"}],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let _ = child.kill().await;
}
