use std::sync::Arc;
use axum::Router;
use axum::routing::{get, post, delete};
use sqlx::SqlitePool;

use crate::engine::model_manager::ModelManager;
use crate::tools::registry::ToolRegistry;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub models: Arc<ModelManager>,
    pub tools: Arc<ToolRegistry>,
    pub http: Arc<reqwest::Client>,
}

pub fn build(state: AppState) -> Router {
    Router::new()
        .route("/models", get(super::models::list_models))
        .route("/models/load", post(super::models::load_model))
        .route("/models/download", post(super::download::download_model))
        .route("/chat", post(super::chat::chat))
        .route("/command", post(super::command::command))
        .route("/tools", get(super::tools::list_tools))
        .route("/tools", post(super::tools::create_tool))
        .route("/tools/{name}", delete(super::tools::delete_tool))
        .route("/sessions", get(super::sessions::list_sessions))
        .route("/sessions/{id}/messages", get(super::sessions::get_messages))
        .with_state(state)
}
