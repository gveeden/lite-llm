use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::ModelBackend;
use crate::engine::litert::LiteRtBackend;
use crate::engine::llama::LlamaModelBackend;
use crate::ffi::Engine;

pub struct ModelManager {
    models: RwLock<HashMap<String, Arc<dyn ModelBackend>>>,
    active: RwLock<Option<String>>,
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            models: RwLock::new(HashMap::new()),
            active: RwLock::new(None),
        }
    }

    /// Load a model from `path`.
    ///
    /// `path` can be:
    ///   - A local file path (`/home/…/model.gguf`, `~/models/model.task`, etc.)
    ///   - A HuggingFace repo ID (`owner/repo`) — the best file is auto-downloaded
    ///   - A HuggingFace repo + file (`owner/repo/filename.gguf`)
    ///
    /// Backend is chosen by the resolved file's extension:
    ///   `.gguf`  → llama.cpp (Vulkan-accelerated when available)
    ///   anything else → LiteRT-LM
    pub fn load(&self, model_id: String, path: &str) -> anyhow::Result<()> {
        let local_path = resolve_path(path)?;

        let backend: Arc<dyn ModelBackend> = if local_path.ends_with(".gguf") {
            Arc::new(LlamaModelBackend::load(&local_path)?)
        } else {
            let engine = Engine::new(&local_path)?;
            Arc::new(LiteRtBackend(Arc::new(engine)))
        };

        self.models.write().unwrap().insert(model_id.clone(), backend);
        *self.active.write().unwrap() = Some(model_id);
        Ok(())
    }

    pub fn get(&self, model_id: &str) -> Option<Arc<dyn ModelBackend>> {
        self.models.read().unwrap().get(model_id).cloned()
    }

    /// Returns the requested model, or the only loaded one, or the active model.
    pub fn resolve(&self, model_id: Option<&str>) -> Option<Arc<dyn ModelBackend>> {
        let models = self.models.read().unwrap();
        if let Some(id) = model_id {
            return models.get(id).cloned();
        }
        if models.len() == 1 {
            return models.values().next().cloned();
        }
        let active = self.active.read().unwrap();
        active.as_deref().and_then(|id| models.get(id).cloned())
    }

    pub fn loaded_ids(&self) -> Vec<String> {
        self.models.read().unwrap().keys().cloned().collect()
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().unwrap().clone()
    }
}

// ── Path resolution ───────────────────────────────────────────────────────────

/// Resolve a model path to a local filesystem path, downloading from
/// HuggingFace if necessary.
fn resolve_path(path: &str) -> anyhow::Result<String> {
    // Local paths: absolute, relative, or starting with ~
    if path.starts_with('/') || path.starts_with('.') || path.starts_with('~') {
        return Ok(shellexpand::tilde(path).into_owned());
    }

    // `owner/repo/filename.ext` — specific file in a HF repo
    let parts: Vec<&str> = path.splitn(3, '/').collect();
    match parts.as_slice() {
        [_owner, _repo, filename] => {
            let repo_id = format!("{}/{}", parts[0], parts[1]);
            tracing::info!("HuggingFace: downloading {filename} from {repo_id}");
            hf_get(&repo_id, filename)
        }
        [_owner, _repo] => {
            // `owner/repo` — pick the best file automatically
            tracing::info!("HuggingFace: resolving best model file from {path}");
            hf_auto_pick(path)
        }
        _ => {
            // Bare filename or unrecognised — treat as local
            Ok(shellexpand::tilde(path).into_owned())
        }
    }
}

/// Download (or use cached) a specific file from a HF repo.
fn hf_get(repo_id: &str, filename: &str) -> anyhow::Result<String> {
    let api = build_hf_api()?;
    let local = api.model(repo_id.to_string()).get(filename)
        .map_err(|e| anyhow::anyhow!("HuggingFace download failed ({repo_id}/{filename}): {e}"))?;
    Ok(local.to_string_lossy().into_owned())
}

/// List a HF repo's files and download the best model file.
///
/// Preference order for GGUF: Q4_K_M → Q4_K_S → Q4_0 → any Q4 → Q8_0 → first .gguf
/// For LiteRT: first .litertlm or .task file.
fn hf_auto_pick(repo_id: &str) -> anyhow::Result<String> {
    let api = build_hf_api()?;
    let repo = api.model(repo_id.to_string());

    let info = repo.info()
        .map_err(|e| anyhow::anyhow!("Failed to list {repo_id}: {e}"))?;

    let filenames: Vec<&str> = info.siblings.iter()
        .map(|s| s.rfilename.as_str())
        .collect();

    // Try GGUF first
    let gguf_files: Vec<&&str> = filenames.iter()
        .filter(|f| f.ends_with(".gguf"))
        .collect();

    if !gguf_files.is_empty() {
        let preferred = ["Q4_K_M", "Q4_K_S", "Q4_0", "Q4", "Q8_0"];
        let filename = preferred.iter()
            .find_map(|q| gguf_files.iter().find(|f| f.contains(q)))
            .or_else(|| gguf_files.first())
            .map(|f| **f)
            .ok_or_else(|| anyhow::anyhow!("No GGUF files in {repo_id}"))?;

        tracing::info!("HuggingFace: selected {filename} from {repo_id}");
        return hf_get(repo_id, filename);
    }

    // Fall back to LiteRT-LM formats
    let litert = filenames.iter()
        .find(|f| f.ends_with(".litertlm") || f.ends_with(".task"));

    if let Some(filename) = litert {
        tracing::info!("HuggingFace: selected {filename} from {repo_id}");
        return hf_get(repo_id, filename);
    }

    anyhow::bail!("No recognised model file (.gguf, .litertlm, .task) found in {repo_id}")
}

fn build_hf_api() -> anyhow::Result<hf_hub::api::sync::Api> {
    let mut builder = hf_hub::api::sync::ApiBuilder::new();
    if let Ok(token) = std::env::var("HF_TOKEN") {
        builder = builder.with_token(Some(token));
    }
    Ok(builder.build()?)
}
