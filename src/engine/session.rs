use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::ffi::{Engine, Conversation};
use crate::tools::{ToolDefinition, executor};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolExecution {
    pub tool: String,
    pub args: Value,
    pub result: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeltaEvent {
    pub delta: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoneEvent {
    pub delta: String,
    pub done: bool,
    pub tool_executions: Vec<ToolExecution>,
}

/// Events sent over the SSE channel.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Delta(DeltaEvent),
    Done(DoneEvent),
}

// ── Session runner ────────────────────────────────────────────────────────────

/// Run a full tool-loop conversation turn.
///
/// Creates a fresh Conversation per call, pre-loaded with `history`.
/// Current datetime is injected into the system prompt automatically.
/// Streams final answer deltas via `tx`.
pub async fn run(
    engine: Arc<Engine>,
    history: Vec<Message>,
    user_message: &str,
    tools: Vec<ToolDefinition>,
    http: Arc<reqwest::Client>,
    tx: UnboundedSender<SessionEvent>,
) -> anyhow::Result<()> {
    let system_json = build_system_message();
    let tools_json = build_tools_json(&tools)?;
    let messages_json = build_messages_json(&history)?;

    let conv = Conversation::new(
        &engine,
        Some(&system_json),
        tools_json.as_deref(),
        messages_json.as_deref(),
        // Constrained decoding requires libGemmaModelConstraintProvider.so at
        // runtime. Disable it for now — Gemma is trained for function calling
        // and produces valid tool call output without it.
        false,
    )?;

    let mut next_msg = build_user_message(user_message);
    let mut tool_executions: Vec<ToolExecution> = Vec::new();

    // Guard against infinite tool-call loops (e.g. wrong result format confusing the model).
    const MAX_TOOL_TURNS: usize = 10;
    let mut tool_turns = 0;

    loop {
        // FFI is blocking — run on the current thread via block_in_place.
        tracing::debug!(msg = %next_msg, "→ send_message");
        let response_json = tokio::task::block_in_place(|| conv.send_message(&next_msg))?;
        tracing::debug!(resp = %response_json, "← response");

        match parse_response(&response_json) {
            ParsedResponse::Text(text) => {
                let _ = tx.send(SessionEvent::Delta(DeltaEvent {
                    delta: text,
                    done: false,
                }));
                let _ = tx.send(SessionEvent::Done(DoneEvent {
                    delta: String::new(),
                    done: true,
                    tool_executions,
                }));
                return Ok(());
            }

            ParsedResponse::ToolCall { name, arguments } => {
                tool_turns += 1;
                if tool_turns > MAX_TOOL_TURNS {
                    anyhow::bail!("Exceeded maximum tool turns ({MAX_TOOL_TURNS}); aborting");
                }

                let tool = tools.iter().find(|t| t.name == name).ok_or_else(|| {
                    anyhow::anyhow!("Model called unknown tool: {name}")
                })?;

                let arguments = clean_gemma_tokens(arguments);
                let result = executor::execute(tool, &arguments, &http)
                    .await
                    .unwrap_or_else(|e| format!("error: {e}"));

                // Large tool results (e.g. full JSON APIs) will exceed the model's
                // context window. Cap at a safe size; the model sees the truncation notice.
                const MAX_RESULT_CHARS: usize = 1500;
                let result = if result.len() > MAX_RESULT_CHARS {
                    format!("{}…[truncated {} chars]", &result[..MAX_RESULT_CHARS], result.len())
                } else {
                    result
                };

                tool_executions.push(ToolExecution {
                    tool: name.clone(),
                    args: arguments,
                    result: result.clone(),
                });

                next_msg = build_tool_result_message(&name, &result);
                // Continue loop — model will now produce a follow-up response.
            }

            ParsedResponse::Unknown(raw) => {
                let _ = tx.send(SessionEvent::Delta(DeltaEvent { delta: raw, done: false }));
                let _ = tx.send(SessionEvent::Done(DoneEvent {
                    delta: String::new(),
                    done: true,
                    tool_executions,
                }));
                return Ok(());
            }
        }
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn build_system_message() -> String {
    let now = chrono::Local::now();
    let text = format!(
        "You are a helpful assistant. \
        The current date is {} and the current time is {} {}. \
        Always use these values when the user asks what day, date, or time it is. \
        Do not say you lack access to real-time information — you have been given the current date and time above.",
        now.format("%A, %B %-d, %Y"),
        now.format("%H:%M"),
        now.format("%Z"),
    );
    serde_json::json!({"type": "text", "text": text}).to_string()
}

fn build_user_message(text: &str) -> String {
    serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": text}]
    })
    .to_string()
}

fn build_tool_result_message(tool_name: &str, result: &str) -> String {
    // Gemma 4 chat template forward-scans role:"tool" messages from the preceding
    // assistant tool_call message. It resolves the function name from follow["name"]
    // and passes content directly to format_tool_response_block, which expects a
    // plain string or mapping (not a {"tool_response":…} wrapper). Content as a
    // string produces: <|tool_response>response:name{value:"result"}<tool_response|>
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

enum ParsedResponse {
    Text(String),
    ToolCall { name: String, arguments: Value },
    Unknown(String),
}

fn parse_response(json_str: &str) -> ParsedResponse {
    let Ok(val): Result<Value, _> = serde_json::from_str(json_str) else {
        return ParsedResponse::Unknown(json_str.to_string());
    };

    // OpenAI format: { "tool_calls": [{"function": {"name": "...", "arguments": "..."}}] }
    if let Some(calls) = val.get("tool_calls").and_then(|v| v.as_array()) {
        if let Some(call) = calls.first() {
            let name = call
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = match call.pointer("/function/arguments") {
                Some(v) if v.is_object() => v.clone(),
                Some(v) if v.is_string() => serde_json::from_str(v.as_str().unwrap_or("{}"))
                    .unwrap_or(Value::Object(Default::default())),
                _ => Value::Object(Default::default()),
            };
            return ParsedResponse::ToolCall { name, arguments };
        }
    }

    // Gemini/Gemma format: { "content": [{"function_call": {"name": "...", "args": {...}}}] }
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
                return ParsedResponse::ToolCall { name, arguments };
            }
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                return ParsedResponse::Text(text.to_string());
            }
        }
    }

    // Bare string response.
    if let Some(text) = val.as_str() {
        return ParsedResponse::Text(text.to_string());
    }

    ParsedResponse::Unknown(json_str.to_string())
}

/// Strip Gemma special-token artifacts (e.g. `<|"|>`) from argument values.
///
/// Without constrained decoding, Gemma 4 wraps JSON string values in raw
/// vocabulary tokens like `<|"` … `"|>`. This walks the parsed Value and
/// removes those markers so substitution templates receive clean strings.
fn clean_gemma_tokens(val: Value) -> Value {
    match val {
        Value::String(s) => Value::String(
            s.replace("<|\"", "").replace("\"|>", "")
             .replace("<|", "").replace("|>", ""),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter().map(|(k, v)| (k, clean_gemma_tokens(v))).collect(),
        ),
        Value::Array(arr) => Value::Array(
            arr.into_iter().map(clean_gemma_tokens).collect(),
        ),
        other => other,
    }
}
