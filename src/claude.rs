//! Anthropic Messages API ↔ OpenAI Chat Completions translation.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

/// Convert Anthropic `/v1/messages` body to OpenAI `/v1/chat/completions` body.
pub fn anthropic_to_openai(body: &Bytes) -> AppResult<Bytes> {
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

    if let Some(system) = obj.get("system") {
        match system {
            Value::String(s) if !s.is_empty() => {
                messages.push(json!({"role": "system", "content": s}));
            }
            Value::Array(blocks) => {
                let text = blocks_to_text(blocks);
                if !text.is_empty() {
                    messages.push(json!({"role": "system", "content": text}));
                }
            }
            _ => {}
        }
    }

    let anth_messages = obj
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| AppError::BadRequest("messages is required".into()))?;

    for m in anth_messages {
        let role = m
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user");
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        let content_val = match content {
            Value::String(s) => Value::String(s),
            Value::Array(blocks) => Value::String(blocks_to_text(&blocks)),
            other => other,
        };
        messages.push(json!({"role": role, "content": content_val}));
    }

    let mut out = json!({
        "model": model,
        "messages": messages,
    });
    let out_obj = out.as_object_mut().unwrap();

    if let Some(mt) = obj.get("max_tokens") {
        out_obj.insert("max_tokens".into(), mt.clone());
    }
    if let Some(t) = obj.get("temperature") {
        out_obj.insert("temperature".into(), t.clone());
    }
    if let Some(t) = obj.get("top_p") {
        out_obj.insert("top_p".into(), t.clone());
    }
    if let Some(s) = obj.get("stop_sequences").and_then(|s| s.as_array()) {
        out_obj.insert("stop".into(), Value::Array(s.clone()));
    }
    let stream = obj
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    out_obj.insert("stream".into(), Value::Bool(stream));

    // tools (basic pass-through shape conversion)
    if let Some(tools) = obj.get("tools").and_then(|t| t.as_array()) {
        let mut oai_tools = Vec::new();
        for t in tools {
            let name = t.get("name").cloned().unwrap_or(json!("tool"));
            let description = t.get("description").cloned().unwrap_or(json!(""));
            let parameters = t
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            oai_tools.push(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters,
                }
            }));
        }
        if !oai_tools.is_empty() {
            out_obj.insert("tools".into(), Value::Array(oai_tools));
        }
    }

    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

fn blocks_to_text(blocks: &[Value]) -> String {
    let mut parts = Vec::new();
    for b in blocks {
        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
            parts.push(t.to_string());
            continue;
        }
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                parts.push(t.to_string());
            }
        } else if let Some(s) = b.as_str() {
            parts.push(s.to_string());
        }
    }
    parts.join("\n")
}

/// Convert OpenAI chat completion JSON to Anthropic messages response.
pub fn openai_to_anthropic(body: &Bytes, model: &str) -> AppResult<Bytes> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("upstream JSON: {e}")))?;

    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("msg_thinrouter");
    let content_text = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let finish = v
        .pointer("/choices/0/finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop");
    let stop_reason = match finish {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        other => other,
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
        "id": id.replace("chatcmpl-", "msg_"),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": content_text}],
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }
    });
    Ok(Bytes::from(serde_json::to_vec(&out)?))
}

/// Convert a single OpenAI SSE `data: {...}` payload to Anthropic SSE events (may be multiple).
/// Returns lines without trailing blank separator handling — caller joins with `\n\n`.
pub fn openai_sse_chunk_to_anthropic(
    data: &str,
    state: &mut StreamTranslateState,
) -> Vec<String> {
    if data.trim() == "[DONE]" {
        let mut events = Vec::new();
        if !state.message_stop_sent {
            events.push(sse_event(
                "message_delta",
                &json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"output_tokens": state.output_tokens}
                }),
            ));
            events.push(sse_event(
                "message_stop",
                &json!({"type": "message_stop"}),
            ));
            state.message_stop_sent = true;
        }
        return events;
    }

    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };

    let mut events = Vec::new();

    if !state.message_start_sent {
        let model = v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(state.model.as_str());
        let id = v
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("msg_thinrouter");
        events.push(sse_event(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": id.replace("chatcmpl-", "msg_"),
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        ));
        events.push(sse_event(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
        ));
        state.message_start_sent = true;
    }

    if let Some(delta) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
        if !delta.is_empty() {
            state.output_tokens += 1; // coarse
            events.push(sse_event(
                "content_block_delta",
                &json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": delta}
                }),
            ));
        }
    }

    if v.pointer("/choices/0/finish_reason")
        .and_then(|f| f.as_str())
        .is_some()
        && !state.block_stop_sent
    {
        events.push(sse_event(
            "content_block_stop",
            &json!({"type": "content_block_stop", "index": 0}),
        ));
        state.block_stop_sent = true;
    }

    events
}

fn sse_event(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}")
}

#[derive(Debug)]
pub struct StreamTranslateState {
    pub model: String,
    pub message_start_sent: bool,
    pub block_stop_sent: bool,
    pub message_stop_sent: bool,
    pub output_tokens: i64,
}

impl StreamTranslateState {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            message_start_sent: false,
            block_stop_sent: false,
            message_stop_sent: false,
            output_tokens: 0,
        }
    }
}

pub fn extract_anthropic_model_stream(body: &Bytes) -> AppResult<(String, bool)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_basic_request() {
        let body = Bytes::from(
            r#"{"model":"cheap","max_tokens":64,"messages":[{"role":"user","content":"hi"}],"system":"be brief"}"#,
        );
        let out = anthropic_to_openai(&body).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "cheap");
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hi");
    }

    #[test]
    fn convert_response() {
        let body = Bytes::from(
            r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#,
        );
        let out = openai_to_anthropic(&body, "cheap").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["type"], "message");
        assert_eq!(v["content"][0]["text"], "hello");
        assert_eq!(v["usage"]["input_tokens"], 3);
    }
}
