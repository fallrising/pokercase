//! Optional request rewrite to cut tool / large message tokens (RTK-inspired, opt-in).

use bytes::Bytes;
use serde_json::Value;

use crate::error::{AppError, AppResult};

/// Apply token-saver rewrites when enabled.
///
/// Rules (OpenAI chat-completions shape):
/// - `role == "tool"` content truncated
/// - `role == "user"` content that looks like huge tool dumps truncated
/// - Anthropic-style content blocks with `type: tool_result` truncated
pub fn maybe_rewrite(body: &Bytes, enabled: bool, max_tool_chars: usize) -> AppResult<Bytes> {
    if !enabled {
        return Ok(body.clone());
    }
    let mut v: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    let Some(obj) = v.as_object_mut() else {
        return Ok(body.clone());
    };

    if let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for msg in messages.iter_mut() {
            compress_message(msg, max_tool_chars);
        }
    }
    // Responses API: input array items
    if let Some(input) = obj.get_mut("input").and_then(|m| m.as_array_mut()) {
        for item in input.iter_mut() {
            compress_message(item, max_tool_chars);
            if let Some(content) = item.get_mut("content").and_then(|c| c.as_array_mut()) {
                for block in content.iter_mut() {
                    compress_block(block, max_tool_chars);
                }
            }
        }
    }

    Ok(Bytes::from(serde_json::to_vec(&v)?))
}

fn compress_message(msg: &mut Value, max_chars: usize) {
    let role = msg
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let is_tool = role == "tool" || role == "function";

    if let Some(content) = msg.get_mut("content") {
        match content {
            Value::String(s) => {
                if is_tool || s.len() > max_chars * 2 {
                    *s = truncate_with_marker(s, max_chars);
                }
            }
            Value::Array(blocks) => {
                for b in blocks.iter_mut() {
                    compress_block(b, max_chars);
                }
            }
            _ => {}
        }
    }
}

fn compress_block(block: &mut Value, max_chars: usize) {
    let ty = block
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let compress = matches!(
        ty.as_str(),
        "tool_result" | "tool_use" | "function_call_output" | "function_call"
    ) || block.get("tool_use_id").is_some();

    if let Some(Value::String(s)) = block.get_mut("text") {
        if compress || s.len() > max_chars * 2 {
            *s = truncate_with_marker(s, max_chars);
        }
    }
    if let Some(Value::String(s)) = block.get_mut("content") {
        if compress || s.len() > max_chars * 2 {
            *s = truncate_with_marker(s, max_chars);
        }
    }
    if let Some(Value::String(s)) = block.get_mut("output") {
        if compress || s.len() > max_chars * 2 {
            *s = truncate_with_marker(s, max_chars);
        }
    }
    // nested content string under tool_result
    if compress {
        if let Some(c) = block.get_mut("content") {
            if let Value::String(s) = c {
                *s = truncate_with_marker(s, max_chars);
            } else if let Value::Array(arr) = c {
                for b in arr.iter_mut() {
                    compress_block(b, max_chars);
                }
            }
        }
    }
}

fn truncate_with_marker(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_chars).collect();
    format!(
        "{kept}\n\n…[thinrouter token-saver truncated {omitted} chars]",
        omitted = count - max_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_tool_role() {
        let big = "x".repeat(5000);
        let body = Bytes::from(
            json!({
                "model": "m",
                "messages": [
                    {"role":"user","content":"hi"},
                    {"role":"tool","content": big}
                ]
            })
            .to_string(),
        );
        let out = maybe_rewrite(&body, true, 100).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let tool = v["messages"][1]["content"].as_str().unwrap();
        assert!(tool.contains("token-saver truncated"));
        assert!(tool.len() < 5000);
    }

    #[test]
    fn disabled_passthrough() {
        let body = Bytes::from(r#"{"model":"m","messages":[{"role":"tool","content":"abc"}]}"#);
        let out = maybe_rewrite(&body, false, 10).unwrap();
        assert_eq!(out, body);
    }
}
