//! Antigravity (Google Code Assist) generateContent adapter.

use bytes::Bytes;
use serde_json::{json, Value};
use tracing::info;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::http_client;

const LOAD_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";

/// Discover `cloudaicompanionProject` via loadCodeAssist.
pub async fn fetch_project_id(access_token: &str) -> AppResult<String> {
    let client = http_client::build_http_client().map_err(AppError::Internal)?;
    let resp = client
        .post(LOAD_URL)
        .header("authorization", format!("Bearer {access_token}"))
        .header("content-type", "application/json")
        .header("user-agent", "antigravity/ide/1.0 darwin/arm64")
        .json(&json!({"metadata": {"ideType": "ANTIGRAVITY"}}))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("loadCodeAssist: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 502,
            body: format!("loadCodeAssist failed: {t}"),
        });
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("loadCodeAssist json: {e}")))?;
    let project = v
        .get("cloudaicompanionProject")
        .and_then(|p| p.as_str())
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("no cloudaicompanionProject in loadCodeAssist")))?;
    info!(%project, "antigravity project resolved");
    Ok(project.to_string())
}

/// Convert OpenAI chat-completions JSON → Antigravity generateContent envelope.
pub fn chat_to_agy_request(
    body: &Bytes,
    project: &str,
    upstream_model: &str,
) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| AppError::BadRequest("body must be object".into()))?;

    let mut contents: Vec<Value> = Vec::new();
    let mut system_parts: Vec<String> = Vec::new();

    if let Some(msgs) = obj.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let text = message_text(m);
            if role == "system" {
                system_parts.push(text);
                continue;
            }
            let g_role = if role == "assistant" { "model" } else { "user" };
            contents.push(json!({
                "role": g_role,
                "parts": [{"text": text}]
            }));
        }
    }
    if contents.is_empty() {
        return Err(AppError::BadRequest("messages required".into()));
    }

    let max_tokens = obj
        .get("max_tokens")
        .and_then(|n| n.as_u64())
        .unwrap_or(2048)
        .min(64000);

    let mut request = json!({
        "contents": contents,
        "generationConfig": {
            "maxOutputTokens": max_tokens,
        },
        "sessionId": Uuid::new_v4().to_string(),
    });
    if !system_parts.is_empty() {
        request.as_object_mut().unwrap().insert(
            "systemInstruction".into(),
            json!({"parts": [{"text": system_parts.join("\n")}]}),
        );
    }

    let sid = request
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("sess");
    let now = chrono::Utc::now().timestamp_millis();
    let traj = Uuid::new_v4();
    let request_id = format!("agent/{sid}/{now}/{traj}/1");

    let model = if upstream_model.is_empty() {
        "gemini-2.5-flash"
    } else {
        upstream_model
    };

    let out = json!({
        "project": project,
        "model": model,
        "userAgent": "antigravity",
        "requestType": "agent",
        "requestId": request_id,
        "request": request,
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

fn message_text(m: &Value) -> String {
    match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Convert Antigravity generateContent response → OpenAI chat completion JSON.
pub fn agy_to_chat_response(body: &Bytes, model: &str) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("upstream JSON: {e}")))?;

    let mut text = String::new();
    if let Some(parts) = v
        .pointer("/response/candidates/0/content/parts")
        .and_then(|p| p.as_array())
    {
        for p in parts {
            if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
        }
    }
    // alternate shapes
    if text.is_empty() {
        if let Some(t) = v
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|t| t.as_str())
        {
            text = t.to_string();
        }
    }

    let prompt = v
        .pointer("/response/usageMetadata/promptTokenCount")
        .and_then(|n| n.as_i64())
        .or_else(|| v.pointer("/usageMetadata/promptTokenCount").and_then(|n| n.as_i64()))
        .unwrap_or(0);
    let completion = v
        .pointer("/response/usageMetadata/candidatesTokenCount")
        .and_then(|n| n.as_i64())
        .or_else(|| {
            v.pointer("/usageMetadata/candidatesTokenCount")
                .and_then(|n| n.as_i64())
        })
        .unwrap_or(0);

    let out = json!({
        "id": format!("chatcmpl-agy-{}", Uuid::new_v4()),
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": prompt + completion
        }
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request() {
        let body = Bytes::from(
            r#"{"model":"x","messages":[{"role":"user","content":"hi"}],"max_tokens":32}"#,
        );
        let out = chat_to_agy_request(&body, "unified-rock-kzb0z", "gemini-2.5-flash").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["project"], "unified-rock-kzb0z");
        assert_eq!(v["model"], "gemini-2.5-flash");
        assert_eq!(v["request"]["contents"][0]["role"], "user");
    }

    #[test]
    fn parse_response() {
        let body = Bytes::from(
            r#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"pong"}]}}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}}}"#,
        );
        let out = agy_to_chat_response(&body, "gemini-2.5-flash").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "pong");
    }
}
