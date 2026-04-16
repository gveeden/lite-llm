use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use crate::api::router::AppState;
use crate::tools::ToolDefinition;

pub async fn list_tools(State(state): State<AppState>) -> Json<Vec<ToolDefinition>> {
    Json(state.tools.all())
}

pub async fn create_tool(
    State(state): State<AppState>,
    Json(tool): Json<ToolDefinition>,
) -> Result<(StatusCode, Json<ToolDefinition>), String> {
    let tool_clone = tool.clone();
    state
        .tools
        .insert(tool)
        .await
        .map_err(|e| e.to_string())?;
    Ok((StatusCode::CREATED, Json(tool_clone)))
}

pub async fn delete_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    match state.tools.delete(&name).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}