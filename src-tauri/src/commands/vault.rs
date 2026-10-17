use somme_application::dto::VaultStatus;
use tauri::State;

use crate::error::ApiResult;
use crate::state::AppState;

#[tauri::command]
pub async fn vault_status(state: State<'_, AppState>) -> ApiResult<VaultStatus> {
    Ok(state.vault.status().await?)
}

#[tauri::command]
pub async fn vault_setup(state: State<'_, AppState>, password: String, remember: bool) -> ApiResult<VaultStatus> {
    Ok(state.vault.setup(&password, remember).await?)
}

#[tauri::command]
pub async fn vault_unlock(state: State<'_, AppState>, password: String, remember: bool) -> ApiResult<VaultStatus> {
    Ok(state.vault.unlock(&password, remember).await?)
}

#[tauri::command]
pub async fn vault_lock(state: State<'_, AppState>) -> ApiResult<VaultStatus> {
    Ok(state.vault.lock().await?)
}
