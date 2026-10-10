// Tauri Commands for model operations — exposed to frontend via invoke()

use crate::models::model::ModelConfig;
use crate::services::model_manager::{self, AddModelInput, UpdateModelInput};

/// Get all models (user + built-in + local)
#[tauri::command]
pub fn get_models() -> Vec<ModelConfig> {
    model_manager::get_models()
}

/// Add a new model
#[tauri::command]
pub fn add_model(input: AddModelInput) -> ModelConfig {
    model_manager::add_model(input)
}

/// Delete a model by internal ID
#[tauri::command]
pub fn delete_model(internal_id: String) -> bool {
    let deleted = model_manager::delete_model(&internal_id);
    if deleted {
        if let Err(error) = crate::services::smart_router::remove_candidate(&internal_id) {
            log::warn!("Failed to remove deleted model from Smart Router: {error}");
            crate::services::smart_router::forget_candidate_memory(&internal_id);
        }
    }
    deleted
}

/// Update a model
#[tauri::command]
pub fn update_model(internal_id: String, updates: UpdateModelInput) -> Option<ModelConfig> {
    let resets_route_memory = updates.base_url.is_some()
        || updates.anthropic_url.is_some()
        || updates.api_key.is_some()
        || updates.model_id.is_some();
    let updated = model_manager::update_model(&internal_id, updates);
    if resets_route_memory && updated.is_some() {
        crate::services::smart_router::forget_candidate_memory(&internal_id);
    }
    updated
}
