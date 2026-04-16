use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use crate::api::router::AppState;

#[derive(Deserialize)]
pub struct LoadModelRequest {
    pub model_id: String,
    pub model_path: String,
}

#[derive(Serialize)]
pub struct LoadModelResponse {
    pub model_handle: String,
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct ListModelsResponse {
    pub loaded: Vec<String>,
    pub active: Option<String>,
}

pub async fn load_model(
    State(state): State<AppState>,
    Json(req): Json<LoadModelRequest>,
) -> Result<Json<LoadModelResponse>, String> {
    state
        .models
        .load(req.model_id.clone(), &req.model_path)
        .map_err(|e| e.to_string())?;

    Ok(Json(LoadModelResponse {
        model_handle: req.model_id,
        status: "loaded",
    }))
}

pub async fn list_models(State(state): State<AppState>) -> Json<ListModelsResponse> {
    Json(ListModelsResponse {
        loaded: state.models.loaded_ids(),
        active: state.models.active_id(),
    })
}
