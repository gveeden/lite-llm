use std::sync::Arc;
use serde_json::Value;

use crate::ffi::{Conversation, Engine};
use crate::tools::ToolDefinition;

use super::{BackendResponse, ConversationHandle, IncomingMessage, Message, ModelBackend};

// ── Backend ───────────────────────────────────────────────────────────────────

pub struct LiteRtBackend(pub Arc<Engine>);

impl ModelBackend for LiteRtBackend {
    fn new_conversation(
        &self,
        system: Option<&str>,
        tools: &[ToolDefinition],
        history: &[Message],
    ) -> anyhow::Result<Box<dyn ConversationHandle>> {
        // LiteRT-LM expects system as {"type":"text","text":"..."} JSON.
        let sys_json = system
            .map(|s| serde_json::json!({"type": "text", "text": s}).to_string());
        let tools_json = build_tools_json(tools)?;
        let messages_json = build_messages_json(history)?;

        let conv = Conversation::new(
            &self.0,
            sys_json.as_deref(),
            tools_json.as_deref(),
            messages_json.as_deref(),
            // Constrained decoding requires libGemmaModelConstraintProvider.so — disabled.
            false,
        )?;

        Ok(Box::new(LiteRtConversation(conv)))
    }
}

// ── Conversation handle ───────────────────────────────────────────────────────

struct LiteRtConversation(Conversation);

impl ConversationHandle for LiteRtConversation {
    fn send(&mut self, msg: IncomingMessage, _delta_tx: Option<&tokio::sync::mpsc::UnboundedSender<String>>) -> anyhow::Result<BackendResponse> {
        let msg_json = match msg {
            IncomingMessage::User(text) => build_user_message(&text),
            IncomingMessage::ToolResult { tool_name, result } => {
                build_tool_result_message(&tool_name, &result)
            }
        };

        tracing::debug!(msg = %msg_json, "→ litert send_message");
        // LiteRT FFI is blocking — must be called from a blocking context.
        let response_json = tokio::task::block_in_place(|| self.0.send_message(&msg_json))?;
        tracing::debug!(resp = %response_json, "← litert response");

        Ok(parse_response(&response_json))
    }
}

// ── JSON message builders ─────────────────────────────────────────────────────

fn build_user_message(text: &str) -> String {
    serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": text}]
    })
    .to_string()
}

fn build_tool_result_message(tool_name: &str, result: &str) -> String {
    // Gemma 4 chat template forward-scans role:"tool" messages from the preceding
    // assistant tool_call. It reads follow["name"] for the function name and passes
    // content directly to format_tool_response_block expecting a plain string.
    serde_json::json!({
        "role": "tool",
        "name": tool_name,
        "content": result,
    })
    .to_string()
}

fn build_tools_json(tools: &[ToolDefinition]) -> anyhow::Result<Option<String>> {
    if tools.is_empty() {
        return Ok(None);
    }
    let arr: Vec<Value> = tools.iter().map(|t| t.to_function_declaration()).collect();
    Ok(Some(serde_json::to_string(&arr)?))
}

fn build_messages_json(history: &[Message]) -> anyhow::Result<Option<String>> {
    if history.is_empty() {
        return Ok(None);
    }
    let msgs: Vec<Value> = history
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": [{"type": "text", "text": m.content}]
            })
        })
        .collect();
    Ok(Some(serde_json::to_string(&msgs)?))
}

// ── Response parsing ──────────────────────────────────────────────────────────

fn parse_response(json_str: &str) -> BackendResponse {
    let Ok(val) = serde_json::from_str::<Value>(json_str) else {
        return BackendResponse::Text(json_str.to_string());
    };

    // OpenAI format: {"tool_calls": [{"function": {"name": "...", "arguments": ...}}]}
    if let Some(calls) = val.get("tool_calls").and_then(|v| v.as_array()) {
        if let Some(call) = calls.first() {
            let name = call
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = match call.pointer("/function/arguments") {
                Some(v) if v.is_object() => clean_gemma_tokens(v.clone()),
                Some(v) if v.is_string() => {
                    let parsed: Value = serde_json::from_str(v.as_str().unwrap_or("{}"))
                        .unwrap_or(Value::Object(Default::default()));
                    clean_gemma_tokens(parsed)
                }
                _ => Value::Object(Default::default()),
            };
            return BackendResponse::ToolCall { name, arguments };
        }
    }

    // Gemma/Gemini format: {"content": [{"function_call": {"name": "...", "args": {...}}}]}
    if let Some(parts) = val.get("content").and_then(|c| c.as_array()) {
        for part in parts {
            if let Some(fc) = part.get("function_call") {
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = fc
                    .get("args")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                return BackendResponse::ToolCall {
                    name,
                    arguments: clean_gemma_tokens(arguments),
                };
            }
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                return BackendResponse::Text(text.to_string());
            }
        }
    }

    // Bare string
    if let Some(text) = val.as_str() {
        return BackendResponse::Text(text.to_string());
    }

    BackendResponse::Text(json_str.to_string())
}

/// Strip Gemma 4 special-token artifacts (`<|"` / `"|>`) from argument values.
///
/// Without constrained decoding, Gemma 4 wraps JSON string values in raw
/// vocabulary tokens. This walks the parsed Value and removes them so
/// tool argument substitution receives clean strings.
fn clean_gemma_tokens(val: Value) -> Value {
    match val {
        Value::String(s) => Value::String(
            s.replace("<|\"", "")
                .replace("\"|>", "")
                .replace("<|", "")
                .replace("|>", ""),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, clean_gemma_tokens(v)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.into_iter().map(clean_gemma_tokens).collect()),
        other => other,
    }
}
