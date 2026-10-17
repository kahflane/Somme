use somme_application::dto::{AccountStatus, SyncStatusView};
use tauri::State;

use crate::error::ApiResult;
use crate::state::AppState;

#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> SyncStatusView {
    state.sync.status()
}

#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> ApiResult<SyncStatusView> {
    Ok(state.sync.sync_now().await?)
}

#[tauri::command]
pub async fn account_status(state: State<'_, AppState>) -> ApiResult<AccountStatus> {
    Ok(state.sync.account())
}

#[tauri::command]
pub async fn account_register(state: State<'_, AppState>, email: String, password: String) -> ApiResult<AccountStatus> {
    Ok(state.sync.register(&email, &password).await?)
}

#[tauri::command]
pub async fn account_login(state: State<'_, AppState>, email: String, password: String) -> ApiResult<AccountStatus> {
    Ok(state.sync.login(&email, &password).await?)
}

#[tauri::command]
pub async fn account_logout(state: State<'_, AppState>) -> ApiResult<AccountStatus> {
    Ok(state.sync.logout().await?)
}
