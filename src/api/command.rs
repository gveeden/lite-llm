use axum::{extract::State, Json};
use axum::response::sse::{Event, Sse};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

use crate::api::router::AppState;
use crate::engine::{session, SessionEvent};

#[derive(Deserialize)]
pub struct CommandRequest {
    pub model: Option<String>,
    pub text: String,
    /// Tool names to enable.
    /// - Omitted or `null`: all registered tools (default)
    /// - `[]`: no tools
    /// - `["name", ...]`: only the named tools
    pub tools: Option<Vec<String>>,
}

pub async fn command(
    State(state): State<AppState>,
    Json(req): Json<CommandRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, String> {
    let engine = state
        .models
        .resolve(req.model.as_deref())
        .ok_or_else(|| "No model loaded".to_string())?;

    let tools = match req.tools {
        None => state.tools.all(),
        Some(ref names) if names.is_empty() => vec![],
        Some(ref names) => state.tools.by_names(names),
    };

    let (tx, rx) = mpsc::unbounded_channel::<SessionEvent>();
    let http = state.http.clone();
    let text = req.text;

    tokio::spawn(async move {
        let _ = session::run(engine, vec![], &text, tools, http, tx).await;
    });

    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        let data = match &event {
            SessionEvent::Delta(d) => serde_json::to_string(d).unwrap_or_default(),
            SessionEvent::Done(d) => serde_json::to_string(d).unwrap_or_default(),
        };
        Ok(Event::default().data(data))
    });

    Ok(Sse::new(stream))
}
