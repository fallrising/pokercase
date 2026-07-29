//! OpenAI Responses API ↔ Chat Completions translation.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

/// Convert Chat Completions request → Responses request (for Codex / Grok upstreams).
pub fn chat_to_responses_request(body: &Bytes) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| AppError::BadRequest("body must be a JSON object".into()))?;

    let model = obj
        .get("model")
        .and_then(|m| m.as_str())
        .ok_or_else(|| AppError::BadRequest("model is required".into()))?;

    let mut instructions: Option<String> = None;
    let mut input: Vec<Value> = Vec::new();

    if let Some(messages) = obj.get("messages").and_then(|m| m.as_array()) {
        for m in messages {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = m.get("content").cloned().unwrap_or(Value::Null);
            let text = match &content {
                Value::String(s) => s.clone(),
                Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => other.to_string(),
            };
            if role == "system" {
                instructions = Some(match instructions {
                    Some(prev) => format!("{prev}\n{text}"),
                    None => text,
                });
            } else {
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": [{"type": "input_text", "text": text}]
                }));
            }
        }
    }

    let mut out = json!({
        "model": model,
        "input": input,
        "store": false,
    });
    let o = out.as_object_mut().unwrap();
    if let Some(instr) = instructions {
        o.insert("instructions".into(), Value::String(instr));
    }
    if let Some(s) = obj.get("stream") {
        o.insert("stream".into(), s.clone());
    }
    if let Some(mt) = obj.get("max_tokens") {
        o.insert("max_output_tokens".into(), mt.clone());
    }
    if let Some(t) = obj.get("temperature") {
        o.insert("temperature".into(), t.clone());
    }
    if let Some(tools) = obj.get("tools") {
        o.insert("tools".into(), tools.clone());
    }

    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

/// Convert Responses JSON response → Chat Completions JSON (non-stream).
pub fn responses_body_to_chat(body: &Bytes) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("upstream JSON: {e}")))?;

    // Prefer structured output[].content[].text
    let mut text = String::new();
    if let Some(output) = v.get("output").and_then(|o| o.as_array()) {
        for item in output {
            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                for block in content {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    if text.is_empty() {
        if let Some(t) = v.pointer("/output_text").and_then(|t| t.as_str()) {
            text = t.to_string();
        }
    }

    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("chatcmpl-from-resp");
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown");
    let input_tokens = v
        .pointer("/usage/input_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);
    let output_tokens = v
        .pointer("/usage/output_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);

    let out = json!({
        "id": id.replace("resp_", "chatcmpl-"),
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens
        }
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

/// Convert Responses request body to Chat Completions.
pub fn responses_to_chat(body: &Bytes) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| AppError::BadRequest("body must be a JSON object".into()))?;

    let model = obj
        .get("model")
        .and_then(|m| m.as_str())
        .ok_or_else(|| AppError::BadRequest("model is required".into()))?;

    let mut messages: Vec<Value> = Vec::new();

    if let Some(instr) = obj.get("instructions").and_then(|i| i.as_str()) {
        if !instr.is_empty() {
            messages.push(json!({"role": "system", "content": instr}));
        }
    }

    match obj.get("input") {
        Some(Value::String(s)) => {
            messages.push(json!({"role": "user", "content": s}));
        }
        Some(Value::Array(items)) => {
            for item in items {
                messages.push(input_item_to_message(item));
            }
        }
        Some(other) => {
            messages.push(json!({"role": "user", "content": other.to_string()}));
        }
        None => {
            return Err(AppError::BadRequest("input is required".into()));
        }
    }

    let mut out = json!({
        "model": model,
        "messages": messages,
    });
    let o = out.as_object_mut().unwrap();

    if let Some(s) = obj.get("stream") {
        o.insert("stream".into(), s.clone());
    }
    if let Some(t) = obj.get("temperature") {
        o.insert("temperature".into(), t.clone());
    }
    if let Some(t) = obj.get("top_p") {
        o.insert("top_p".into(), t.clone());
    }
    if let Some(mt) = obj.get("max_output_tokens").or_else(|| obj.get("max_tokens")) {
        o.insert("max_tokens".into(), mt.clone());
    }
    if let Some(tools) = obj.get("tools") {
        o.insert("tools".into(), tools.clone());
    }

    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

fn input_item_to_message(item: &Value) -> Value {
    let role = item
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("user");
    let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("");

    if ty == "function_call_output" || ty == "tool_result" {
        let content = item
            .get("output")
            .or_else(|| item.get("content"))
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let content = match content {
            Value::String(s) => s,
            other => other.to_string(),
        };
        return json!({"role": "tool", "content": content});
    }

    let content = match item.get("content") {
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for b in blocks {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                } else if let Some(t) = b.as_str() {
                    parts.push(t.to_string());
                }
            }
            Value::String(parts.join("\n"))
        }
        Some(other) => other.clone(),
        None => item
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new())),
    };

    let oai_role = match role {
        "assistant" | "system" | "user" | "tool" => role,
        _ => "user",
    };
    json!({"role": oai_role, "content": content})
}

/// Convert Chat Completions JSON to Responses API shape.
pub fn chat_to_responses(body: &Bytes, model: &str) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("upstream JSON: {e}")))?;

    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("resp_thinrouter");
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let finish = v
        .pointer("/choices/0/finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop");
    let status = match finish {
        "stop" => "completed",
        "length" => "incomplete",
        _ => "completed",
    };
    let input_tokens = v
        .pointer("/usage/prompt_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);
    let output_tokens = v
        .pointer("/usage/completion_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);

    let out = json!({
        "id": id.replace("chatcmpl-", "resp_"),
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": status,
        "model": model,
        "output": [{
            "type": "message",
            "id": "msg_0",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": text
            }]
        }],
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens
        }
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

pub fn extract_responses_model_stream(body: &Bytes) -> AppResult<(String, bool)> {
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

/// Map OpenAI chat SSE chunk to a minimal Responses streaming event (text delta).
pub fn chat_sse_to_responses_events(data: &str, started: &mut bool) -> Vec<String> {
    if data.trim() == "[DONE]" {
        return vec![
            sse("response.completed", &json!({"type": "response.completed"})),
            "data: [DONE]".into(),
        ];
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    if !*started {
        events.push(sse(
            "response.created",
            &json!({
                "type": "response.created",
                "response": {"id": v.get("id").and_then(|i| i.as_str()).unwrap_or("resp_tr"), "status": "in_progress"}
            }),
        ));
        *started = true;
    }
    if let Some(delta) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
        if !delta.is_empty() {
            events.push(sse(
                "response.output_text.delta",
                &json!({
                    "type": "response.output_text.delta",
                    "delta": delta
                }),
            ));
        }
    }
    events
}

fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_string_input() {
        let body = Bytes::from(r#"{"model":"cheap","input":"hello","max_output_tokens":32}"#);
        let out = responses_to_chat(&body).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "cheap");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "hello");
        assert_eq!(v["max_tokens"], 32);
    }

    #[test]
    fn convert_response() {
        let body = Bytes::from(
            r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"yo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        );
        let out = chat_to_responses(&body, "cheap").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["object"], "response");
        assert_eq!(v["output"][0]["content"][0]["text"], "yo");
    }
}
