use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use sqlx::Row;
use crate::api::router::AppState;

#[derive(Serialize)]
pub struct SessionRow {
    pub id: String,
    pub model_id: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_used: i64,
}

#[derive(Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub tool_call: Option<String>,
    pub tool_result: Option<String>,
    pub created_at: i64,
}

pub async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<Vec<SessionRow>>, String> {
    let rows = sqlx::query(
        "SELECT id, model_id, title, created_at, last_used
         FROM sessions ORDER BY last_used DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let sessions = rows
        .into_iter()
        .map(|r| SessionRow {
            id: r.get("id"),
            model_id: r.get("model_id"),
            title: r.get("title"),
            created_at: r.get("created_at"),
            last_used: r.get("last_used"),
        })
        .collect();

    Ok(Json(sessions))
}

pub async fn get_messages(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<MessageRow>>, String> {
    let rows = sqlx::query(
        "SELECT id, session_id, role, content, tool_call, tool_result, created_at
         FROM messages WHERE session_id = ? ORDER BY id ASC",
    )
    .bind(&session_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let messages = rows
        .into_iter()
        .map(|r| MessageRow {
            id: r.get("id"),
            session_id: r.get("session_id"),
            role: r.get("role"),
            content: r.get("content"),
            tool_call: r.get("tool_call"),
            tool_result: r.get("tool_result"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(Json(messages))
}
