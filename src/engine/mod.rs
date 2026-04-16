pub mod litert;
pub mod llama;
pub mod model_manager;
pub mod session;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::ToolDefinition;

// ── Shared event / message types ──────────────────────────────────────────────

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

/// Events streamed over the SSE channel from a running session.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Delta(DeltaEvent),
    Done(DoneEvent),
}

// ── Backend abstraction ───────────────────────────────────────────────────────

/// A message being sent into a conversation.
pub enum IncomingMessage {
    /// Initial or follow-up user turn.
    User(String),
    /// Tool execution result fed back to the model.
    ToolResult { tool_name: String, result: String },
}

/// A decoded response from the model.
pub enum BackendResponse {
    /// The model produced a text answer — conversation is complete.
    Text(String),
    /// The model wants to call a tool before answering.
    ToolCall { name: String, arguments: Value },
}

/// A live, stateful conversation handle. State (KV cache or explicit history)
/// is maintained across successive `send` calls.
pub trait ConversationHandle: Send {
    /// Send a message and return the model's response.
    ///
    /// If `delta_tx` is `Some`, text tokens are forwarded to it as they are
    /// generated so the caller can stream them to the client in real time.
    /// Backends that do not support token-level streaming may ignore it.
    fn send(
        &mut self,
        msg: IncomingMessage,
        delta_tx: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> anyhow::Result<BackendResponse>;
}

/// A loaded model capable of spawning independent conversations.
pub trait ModelBackend: Send + Sync {
    /// `system` is a plain-text string (datetime-injected by the session runner).
    fn new_conversation(
        &self,
        system: Option<&str>,
        tools: &[ToolDefinition],
        history: &[Message],
    ) -> anyhow::Result<Box<dyn ConversationHandle>>;
}
