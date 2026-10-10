// Tauri Commands for the free-model catalog and in-app self-update.

/// Remote-updatable free-model catalog. Returns `null` when both the remote
/// and the on-disk cache are unavailable, so the caller keeps whatever
/// catalog it already has.
#[tauri::command]
pub async fn get_free_model_directory() -> serde_json::Value {
    crate::services::free_model_directory::fetch_free_model_directory().await
}

// ─── In-app self-update (Windows): download installer + launch + exit ───

#[tauri::command]
pub async fn download_and_install_update(
    app_handle: tauri::AppHandle,
    version: String,
) -> Result<(), String> {
    crate::services::self_update::download_and_install(app_handle, version).await
}
