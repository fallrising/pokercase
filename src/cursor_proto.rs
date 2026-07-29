//! Minimal protobuf + Connect-RPC framing for Cursor AgentService.

use std::collections::HashMap;

const WIRE_VARINT: u8 = 0;
const WIRE_LEN: u8 = 2;

pub fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 0x80 {
        bytes.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

pub fn encode_field_varint(field: u32, value: u64) -> Vec<u8> {
    let mut out = encode_varint(((field as u64) << 3) | WIRE_VARINT as u64);
    out.extend(encode_varint(value));
    out
}

pub fn encode_field_bytes(field: u32, data: &[u8]) -> Vec<u8> {
    let mut out = encode_varint(((field as u64) << 3) | WIRE_LEN as u64);
    out.extend(encode_varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

pub fn encode_field_str(field: u32, s: &str) -> Vec<u8> {
    encode_field_bytes(field, s.as_bytes())
}

pub fn encode_field_msg(field: u32, msg: &[u8]) -> Vec<u8> {
    encode_field_bytes(field, msg)
}

pub fn wrap_connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0x00); // no compression
    let len = payload.len() as u32;
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[derive(Debug, Clone)]
pub struct FieldVal {
    #[allow(dead_code)]
    pub wire: u8,
    pub bytes: Vec<u8>,
    #[allow(dead_code)]
    pub varint: Option<u64>,
}

pub type Msg = HashMap<u32, Vec<FieldVal>>;

pub fn decode_varint(buf: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    while pos < buf.len() {
        let b = buf[pos];
        pos += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

pub fn decode_message(data: &[u8]) -> Msg {
    let mut fields = Msg::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some((tag, p1)) = decode_varint(data, pos) else {
            break;
        };
        let field = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        pos = p1;
        match wire {
            WIRE_VARINT => {
                let Some((v, p2)) = decode_varint(data, pos) else {
                    break;
                };
                pos = p2;
                fields.entry(field).or_default().push(FieldVal {
                    wire,
                    bytes: Vec::new(),
                    varint: Some(v),
                });
            }
            WIRE_LEN => {
                let Some((len, p2)) = decode_varint(data, pos) else {
                    break;
                };
                pos = p2;
                let end = pos + len as usize;
                if end > data.len() {
                    break;
                }
                let slice = data[pos..end].to_vec();
                pos = end;
                fields.entry(field).or_default().push(FieldVal {
                    wire,
                    bytes: slice,
                    varint: None,
                });
            }
            1 => {
                // fixed64
                if pos + 8 > data.len() {
                    break;
                }
                let slice = data[pos..pos + 8].to_vec();
                pos += 8;
                fields.entry(field).or_default().push(FieldVal {
                    wire,
                    bytes: slice,
                    varint: None,
                });
            }
            5 => {
                if pos + 4 > data.len() {
                    break;
                }
                let slice = data[pos..pos + 4].to_vec();
                pos += 4;
                fields.entry(field).or_default().push(FieldVal {
                    wire,
                    bytes: slice,
                    varint: None,
                });
            }
            _ => break,
        }
    }
    fields
}

pub fn first_bytes(msg: &Msg, field: u32) -> Option<&[u8]> {
    msg.get(&field)?.first().map(|f| f.bytes.as_slice())
}

pub fn first_str(msg: &Msg, field: u32) -> Option<String> {
    first_bytes(msg, field).map(|b| String::from_utf8_lossy(b).into_owned())
}

pub fn first_msg(msg: &Msg, field: u32) -> Option<Msg> {
    first_bytes(msg, field).map(decode_message)
}

/// Build AgentService Run request frame for text-only chat (9router-compatible).
pub fn build_agent_run_frame(messages: &[(String, String)], model: &str) -> Vec<u8> {
    // messages: (role, content)
    let system: String = messages
        .iter()
        .filter(|(r, _)| r == "system")
        .map(|(_, c)| c.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let chat: Vec<&(String, String)> = messages.iter().filter(|(r, _)| r != "system").collect();
    let current_idx = chat
        .iter()
        .rposition(|(r, _)| r == "user")
        .unwrap_or(chat.len().saturating_sub(1));
    let history = &chat[..current_idx];
    let current = chat.get(current_idx);
    let user_text = current
        .map(|(_, c)| c.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Continue.");

    // ConversationHistoryMessage encoding (from 9router):
    // user: msg(1, msg(1, msg(1, string(1,text))))
    // assistant: msg(2, msg(1, msg(1, string(1,text))))
    let mut history_entries = Vec::new();
    for (role, content) in history {
        if content.is_empty() {
            continue;
        }
        let text = encode_field_str(1, content);
        let content_msg = encode_field_msg(1, &encode_field_msg(1, &text));
        let entry = if role == "assistant" {
            encode_field_msg(2, &content_msg)
        } else {
            encode_field_msg(1, &content_msg)
        };
        history_entries.push(encode_field_msg(1, &entry));
    }
    let conversation_history: Option<Vec<u8>> = if history_entries.is_empty() {
        None
    } else {
        Some(history_entries.concat())
    };

    let user_message = [
        encode_field_str(1, user_text),
        encode_field_str(2, &uuid::Uuid::new_v4().to_string()),
    ]
    .concat();
    let mut user_action = encode_field_msg(1, &user_message);
    if let Some(hist) = conversation_history {
        user_action.extend(encode_field_msg(7, &hist));
    }
    let conversation_action = encode_field_msg(1, &user_action);
    let requested_model = [
        encode_field_str(1, model),
        encode_field_varint(7, 1), // bool true
    ]
    .concat();

    let mut run_request = encode_field_msg(1, &[]); // empty ConversationState
    run_request.extend(encode_field_msg(2, &conversation_action));
    if !system.is_empty() {
        run_request.extend(encode_field_str(8, &system));
    }
    run_request.extend(encode_field_msg(9, &requested_model));

    // AgentClientMessage.run_request = field 1
    let client_msg = encode_field_msg(1, &run_request);
    wrap_connect_frame(&client_msg)
}

/// Empty RequestContext response when AgentService asks for IDE context.
pub fn build_request_context_response() -> Vec<u8> {
    // requestContextSuccess = msg(1, empty)
    // requestContextResult = msg(1, success)
    // execClientMessage = msg(10, result)
    // AgentClientMessage = msg(2, exec)
    let success = encode_field_msg(1, &[]);
    let result = encode_field_msg(1, &success);
    let exec = encode_field_msg(10, &result);
    let client = encode_field_msg(2, &exec);
    wrap_connect_frame(&client)
}

/// Parse AgentServer frames from a buffer; returns remaining incomplete bytes.
pub fn decode_agent_frames(mut pending: Vec<u8>, mut on_payload: impl FnMut(&[u8])) -> Vec<u8> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    loop {
        if pending.len() < 5 {
            break;
        }
        let flags = pending[0];
        let length = u32::from_be_bytes([pending[1], pending[2], pending[3], pending[4]]) as usize;
        if pending.len() < 5 + length {
            break;
        }
        let mut payload = pending[5..5 + length].to_vec();
        pending = pending[5 + length..].to_vec();
        // trailer frames skip
        if flags & 0x02 != 0 {
            continue;
        }
        if flags & 0x01 != 0 {
            let mut dec = GzDecoder::new(&payload[..]);
            let mut out = Vec::new();
            if dec.read_to_end(&mut out).is_ok() {
                payload = out;
            }
        }
        on_payload(&payload);
    }
    pending
}

/// Extract text deltas from an AgentServerMessage payload.
pub fn extract_text_delta(payload: &[u8]) -> (Option<String>, bool, bool) {
    // returns (text, done, needs_context)
    let server = decode_message(payload);
    let mut text = None;
    let mut done = false;
    let mut needs_context = false;

    // field 1: interaction_update
    if let Some(update) = first_msg(&server, 1) {
        // field 1 of update: text delta wrapper
        if let Some(delta_msg) = first_msg(&update, 1) {
            text = first_str(&delta_msg, 1);
        }
        // field 14: turn complete (9router)
        if update.contains_key(&14) {
            done = true;
        }
    }
    // field 2: exec request (IDE tools / context)
    if let Some(exec) = first_msg(&server, 2) {
        if exec.contains_key(&10) {
            needs_context = true;
        } else {
            // unsupported tool
            done = true;
        }
    }
    (text, done, needs_context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_len() {
        let payload = encode_field_str(1, "hi");
        let frame = wrap_connect_frame(&payload);
        assert_eq!(frame[0], 0);
        let len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
        assert_eq!(len, payload.len());
        assert_eq!(&frame[5..], payload.as_slice());
    }

    #[test]
    fn build_run_not_empty() {
        let msgs = vec![("user".into(), "hello".into())];
        let f = build_agent_run_frame(&msgs, "default");
        assert!(f.len() > 10);
    }
}
