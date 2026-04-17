use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

use crate::tools::{executor, ResponseMode, ToolDefinition};

use super::{
    BackendResponse, DeltaEvent, DoneEvent, IncomingMessage, Message, ModelBackend, SessionEvent,
    ToolExecution,
};

/// Run a full tool-loop conversation turn.
///
/// Creates a fresh conversation from `history`, injects the current datetime
/// into the system prompt, then drives the tool loop until the model produces
/// a plain-text answer.  Text tokens are streamed over `tx` as they are
/// generated; tool-call markup is suppressed from the stream.
pub async fn run(
    backend: Arc<dyn ModelBackend>,
    history: Vec<Message>,
    user_message: &str,
    tools: Vec<ToolDefinition>,
    http: Arc<reqwest::Client>,
    tx: UnboundedSender<SessionEvent>,
) -> anyhow::Result<()> {
    let t_e2e = std::time::Instant::now();

    let mut conv = backend.new_conversation(Some(build_system_message()), &tools, &history)?;

    let mut tool_executions: Vec<ToolExecution> = Vec::new();
    const MAX_TOOL_TURNS: usize = 10;
    let mut tool_turns = 0;

    let mut next_msg = Some(IncomingMessage::User(user_message.to_string()));

    loop {
        let msg = next_msg.take().expect("msg always set before loop iteration");

        // ── Per-turn streaming setup ──────────────────────────────────────────
        // piece_tx is passed into the blocking backend; the forwarder task
        // relays each piece to the SSE channel in real time.
        let (piece_tx, mut piece_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let tx2 = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(piece) = piece_rx.recv().await {
                let _ = tx2.send(SessionEvent::Delta(DeltaEvent {
                    delta: piece,
                    done: false,
                }));
            }
        });

        let response =
            tokio::task::block_in_place(|| conv.send(msg, Some(&piece_tx)))?;

        // Close the sender so the forwarder task exits, then wait for it to
        // flush any remaining buffered pieces before we inspect the response.
        drop(piece_tx);
        forwarder.await.ok();

        match response {
            BackendResponse::Text(_) => {
                tracing::info!(e2e_ms = t_e2e.elapsed().as_millis(), "session e2e");
                let _ = tx.send(SessionEvent::Done(DoneEvent {
                    delta: String::new(),
                    done: true,
                    tool_executions,
                }));
                return Ok(());
            }

            BackendResponse::ToolCall { name, arguments } => {
                tool_turns += 1;
                if tool_turns > MAX_TOOL_TURNS {
                    anyhow::bail!("Exceeded maximum tool turns ({MAX_TOOL_TURNS}); aborting");
                }

                let tool = tools
                    .iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| anyhow::anyhow!("Model called unknown tool: {name}"))?;

                let result = executor::execute(tool, &arguments, &http)
                    .await
                    .unwrap_or_else(|e| format!("error: {e}"));

                // Cap large tool results to prevent context window overflow.
                const MAX_RESULT_CHARS: usize = 1500;
                let result = if result.len() > MAX_RESULT_CHARS {
                    format!(
                        "{}…[truncated {} chars]",
                        &result[..MAX_RESULT_CHARS],
                        result.len()
                    )
                } else {
                    result
                };

                tool_executions.push(ToolExecution {
                    tool: name.clone(),
                    args: arguments,
                    result: result.clone(),
                });

                let is_error = result.starts_with("error:");
                let mode = if is_error {
                    &tool.response.on_error
                } else {
                    &tool.response.on_success
                };

                match mode {
                    ResponseMode::Direct => {
                        // Stream the raw result straight to the client.
                        tracing::info!(e2e_ms = t_e2e.elapsed().as_millis(), "session e2e");
                        let _ = tx.send(SessionEvent::Delta(DeltaEvent {
                            delta: result,
                            done: false,
                        }));
                        let _ = tx.send(SessionEvent::Done(DoneEvent {
                            delta: String::new(),
                            done: true,
                            tool_executions,
                        }));
                        return Ok(());
                    }
                    ResponseMode::Llm => {
                        next_msg = Some(IncomingMessage::ToolResult {
                            tool_name: name,
                            result,
                        });
                    }
                }
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_system_message() -> &'static str {
    "You are a helpful assistant. \
     Use the get_datetime tool whenever the user asks for the current date or time. \
     After every tool call, always reply to the user with a short text message summarising the result."
}
