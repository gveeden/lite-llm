use axum::{extract::State, Json};
use axum::response::sse::{Event, Sse};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

use crate::api::router::AppState;
use crate::engine::{session, Message, SessionEvent};

#[derive(Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<String>,
}

pub async fn chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, String> {
    let engine = state
        .models
        .resolve(req.model.as_deref())
        .ok_or_else(|| "No model loaded".to_string())?;

    let tools = if req.tools.is_empty() {
        vec![]
    } else {
        state.tools.by_names(&req.tools)
    };

    let history = req.messages;
    let user_text = history
        .last()
        .filter(|m| m.role == "user")
        .map(|m| m.content.clone())
        .ok_or("Last message must be from user")?;
    let history = history[..history.len() - 1].to_vec();

    let (tx, rx) = mpsc::unbounded_channel::<SessionEvent>();
    let http = state.http.clone();
    let memory = state.memory.clone();

    tokio::spawn(async move {
        let _ = session::run(engine, history, &user_text, tools, http, memory, tx).await;
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
