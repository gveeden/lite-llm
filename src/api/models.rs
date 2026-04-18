use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use crate::api::router::AppState;

#[derive(Deserialize)]
pub struct LoadModelRequest {
    pub model_id: String,
    pub model_path: String,
    pub context_size: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub gpu_layers: Option<u32>,
    pub kv_quant: Option<String>,
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
    let mut cfg = crate::config::ModelConfig::from_path(req.model_path.clone());
    if let Some(v) = req.context_size { cfg.context_size = v; }
    if let Some(v) = req.temperature { cfg.temperature = v; }
    if let Some(v) = req.top_p { cfg.top_p = v; }
    if let Some(v) = req.top_k { cfg.top_k = v; }
    if let Some(v) = req.gpu_layers { cfg.gpu_layers = v; }
    if let Some(v) = req.kv_quant { cfg.kv_quant = Some(v); }

    state
        .models
        .load(req.model_id.clone(), &cfg)
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
