//! Cursor AgentService (HTTP/2 Connect+proto) text chat.

use std::sync::Arc;

use bytes::Bytes;
use http::Request;
use rustls::pki_types::ServerName;
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::cursor_proto::{
    build_agent_run_frame, build_request_context_response, decode_agent_frames, extract_text_delta,
};
use crate::error::{AppError, AppResult};

const AGENT_HOST: &str = "agent.api5.cursor.sh";
const AGENT_PATH: &str = "/agent.v1.AgentService/Run";

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

/// Jyh cipher checksum used by Cursor IDE.
fn cursor_checksum(machine_id: &str) -> String {
    let timestamp = (chrono::Utc::now().timestamp_millis() / 1_000_000) as u64;
    let mut byte_array = [
        ((timestamp >> 40) & 0xff) as u8,
        ((timestamp >> 32) & 0xff) as u8,
        ((timestamp >> 24) & 0xff) as u8,
        ((timestamp >> 16) & 0xff) as u8,
        ((timestamp >> 8) & 0xff) as u8,
        (timestamp & 0xff) as u8,
    ];
    let mut t: u8 = 165;
    for (i, b) in byte_array.iter_mut().enumerate() {
        *b = ((*b ^ t) as u16 + (i as u16 % 256)) as u8;
        t = *b;
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::new();
    let mut i = 0;
    while i < byte_array.len() {
        let a = byte_array[i];
        let b = if i + 1 < byte_array.len() {
            byte_array[i + 1]
        } else {
            0
        };
        let c = if i + 2 < byte_array.len() {
            byte_array[i + 2]
        } else {
            0
        };
        encoded.push(alphabet[(a >> 2) as usize] as char);
        encoded.push(alphabet[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        if i + 1 < byte_array.len() {
            encoded.push(alphabet[(((b & 15) << 2) | (c >> 6)) as usize] as char);
        }
        if i + 2 < byte_array.len() {
            encoded.push(alphabet[(c & 63) as usize] as char);
        }
        i += 3;
    }
    format!("{encoded}{machine_id}")
}

fn clean_token(token: &str) -> &str {
    token
        .split_once("::")
        .map(|(_, t)| t)
        .unwrap_or(token)
}

pub fn build_cursor_headers(access_token: &str, machine_id: Option<&str>) -> Vec<(String, String)> {
    let clean = clean_token(access_token);
    let machine = machine_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| sha256_hex(&format!("{clean}machineId")));
    let session_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, clean.as_bytes()).to_string();
    let client_key = sha256_hex(clean);
    let checksum = cursor_checksum(&machine);
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    };

    vec![
        ("authorization".into(), format!("Bearer {clean}")),
        ("connect-accept-encoding".into(), "gzip".into()),
        ("connect-protocol-version".into(), "1".into()),
        ("content-type".into(), "application/connect+proto".into()),
        ("user-agent".into(), "connect-es/1.6.1".into()),
        (
            "x-amzn-trace-id".into(),
            format!("Root={}", Uuid::new_v4()),
        ),
        ("x-client-key".into(), client_key),
        ("x-cursor-checksum".into(), checksum),
        ("x-cursor-client-version".into(), "3.12.17".into()),
        (
            "x-cursor-client-commit".into(),
            "0fb762053c34788bb7760d5673f8a6d4c8589d50".into(),
        ),
        ("x-cursor-client-type".into(), "ide".into()),
        ("x-cursor-client-os".into(), os.into()),
        ("x-cursor-client-arch".into(), arch.into()),
        ("x-cursor-client-device-type".into(), "desktop".into()),
        (
            "x-cursor-config-version".into(),
            Uuid::new_v4().to_string(),
        ),
        ("x-cursor-timezone".into(), "UTC".into()),
        ("x-ghost-mode".into(), "true".into()),
        ("x-request-id".into(), Uuid::new_v4().to_string()),
        ("x-session-id".into(), session_id),
    ]
}

/// Run a text-only AgentService chat; return OpenAI-shaped JSON bytes.
pub async fn agent_chat_completions(
    access_token: &str,
    machine_id: Option<&str>,
    model: &str,
    messages: &[(String, String)],
) -> AppResult<Bytes> {
    let headers = build_cursor_headers(access_token, machine_id);
    let run_frame = build_agent_run_frame(messages, model);

    // TLS + HTTP/2
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from(AGENT_HOST.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("server name: {e}")))?;

    let tcp = TcpStream::connect((AGENT_HOST, 443))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("tcp connect: {e}")))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("tls: {e}")))?;

    let (mut sender, conn) = h2::client::handshake(tls)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("h2 handshake: {e}")))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            warn!(error = %e, "cursor h2 connection error");
        }
    });

    let mut req = Request::builder()
        .method("POST")
        .uri(format!("https://{AGENT_HOST}{AGENT_PATH}"));
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let request = req
        .body(())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("request build: {e}")))?;

    let (response_future, mut send_stream) = sender
        .send_request(request, false)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("h2 send_request: {e}")))?;

    send_stream
        .send_data(Bytes::from(run_frame), false)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("h2 send frame: {e}")))?;

    let response = response_future
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("h2 response: {e}")))?;
    let status = response.status().as_u16();
    let mut body = response.into_body();

    if status != 200 {
        let mut err = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| AppError::Internal(anyhow::anyhow!("h2 read: {e}")))?;
            err.extend_from_slice(&chunk);
        }
        return Err(AppError::Upstream {
            status,
            body: String::from_utf8_lossy(&err).into_owned(),
        });
    }

    let mut pending = Vec::new();
    let mut content = String::new();
    let mut finished = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);

    loop {
        if finished || tokio::time::Instant::now() > deadline {
            break;
        }
        let next = tokio::time::timeout(std::time::Duration::from_secs(60), body.data()).await;
        match next {
            Ok(Some(Ok(chunk))) => {
                pending.extend_from_slice(&chunk);
                let mut send_ctx = false;
                pending = decode_agent_frames(pending, |payload| {
                    let (text, done, needs_ctx) = extract_text_delta(payload);
                    if let Some(t) = text {
                        content.push_str(&t);
                    }
                    if needs_ctx {
                        send_ctx = true;
                    }
                    if done {
                        finished = true;
                    }
                });
                if send_ctx {
                    let ctx = build_request_context_response();
                    if let Err(e) = send_stream.send_data(Bytes::from(ctx), false) {
                        warn!(error = %e, "failed to send request context");
                    }
                }
            }
            Ok(Some(Err(e))) => {
                return Err(AppError::Internal(anyhow::anyhow!("h2 body: {e}")));
            }
            Ok(None) => break,
            Err(_) => {
                warn!("cursor agent stream timeout");
                break;
            }
        }
    }
    // half-close send side
    let _ = send_stream.send_data(Bytes::new(), true);

    if content.is_empty() && !finished {
        debug!(pending_len = pending.len(), "cursor agent empty content");
    }

    let out = serde_json::json!({
        "id": format!("chatcmpl-cursor-{}", Uuid::new_v4()),
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0
        }
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

/// Parse OpenAI chat JSON body into (role, content) list.
pub fn parse_chat_messages(body: &Bytes) -> AppResult<Vec<(String, String)>> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")))?;
    let msgs = v
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| AppError::BadRequest("messages required".into()))?;
    let mut out = Vec::new();
    for m in msgs {
        let role = m
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user")
            .to_string();
        let content = match m.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        out.push((role, content));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_nonempty() {
        let c = cursor_checksum("abc");
        assert!(c.len() > 10);
    }

    #[test]
    fn headers_have_auth() {
        let h = build_cursor_headers("testtoken", Some("mid"));
        assert!(h.iter().any(|(k, v)| k == "authorization" && v.contains("testtoken")));
    }
}
