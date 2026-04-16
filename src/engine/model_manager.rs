use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::ffi::Engine;

pub struct ModelManager {
    models: RwLock<HashMap<String, Arc<Engine>>>,
    active: RwLock<Option<String>>,
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            models: RwLock::new(HashMap::new()),
            active: RwLock::new(None),
        }
    }

    pub fn load(&self, model_id: String, model_path: &str) -> anyhow::Result<()> {
        let engine = Engine::new(model_path)?;
        let mut models = self.models.write().unwrap();
        models.insert(model_id.clone(), Arc::new(engine));
        *self.active.write().unwrap() = Some(model_id);
        Ok(())
    }

    pub fn get(&self, model_id: &str) -> Option<Arc<Engine>> {
        self.models.read().unwrap().get(model_id).cloned()
    }

    /// Returns the active engine, or the only loaded one, or None.
    pub fn resolve(&self, model_id: Option<&str>) -> Option<Arc<Engine>> {
        let models = self.models.read().unwrap();
        if let Some(id) = model_id {
            return models.get(id).cloned();
        }
        // If exactly one model is loaded, use it.
        if models.len() == 1 {
            return models.values().next().cloned();
        }
        // Otherwise use the active model.
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
