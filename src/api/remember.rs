use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;

#[derive(Deserialize)]
pub struct RememberRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct RememberResponse {
    pub result: String,
}

pub async fn remember(
    State(state): State<AppState>,
    Json(req): Json<RememberRequest>,
) -> Result<Json<RememberResponse>, (StatusCode, String)> {
    let store = state.memory.as_ref().ok_or_else(|| {
        (StatusCode::SERVICE_UNAVAILABLE, "memory is disabled".to_string())
    })?;

    let result = store
        .insert(&req.text)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(RememberResponse {
        result: result.to_string(),
    }))
}

#[derive(Serialize)]
pub struct MemoryEntry {
    pub content: String,
}

pub async fn list_memories(
    State(state): State<AppState>,
) -> Result<Json<Vec<MemoryEntry>>, (StatusCode, String)> {
    let store = state.memory.as_ref().ok_or_else(|| {
        (StatusCode::SERVICE_UNAVAILABLE, "memory is disabled".to_string())
    })?;

    let entries = store
        .list()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(
        entries.into_iter().map(|content| MemoryEntry { content }).collect(),
    ))
}
