use somme_application::dto::{
    GroupInput, GroupView, HostInput, HostView, IdentityInput, IdentityView, KeyGenerateInput,
    KeyImportInput, KeyView, KnownHostView, PortForwardInput, PortForwardView, SnippetInput,
    SnippetView,
};
use somme_domain::Id;
use tauri::State;

use crate::error::ApiResult;
use crate::state::AppState;

#[tauri::command]
pub async fn groups_list(state: State<'_, AppState>) -> ApiResult<Vec<GroupView>> {
    Ok(state.groups.list().await?)
}

#[tauri::command]
pub async fn group_save(state: State<'_, AppState>, input: GroupInput) -> ApiResult<GroupView> {
    Ok(state.groups.save(input).await?)
}

#[tauri::command]
pub async fn group_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.groups.delete(id).await?)
}

#[tauri::command]
pub async fn hosts_list(state: State<'_, AppState>) -> ApiResult<Vec<HostView>> {
    Ok(state.hosts.list().await?)
}

#[tauri::command]
pub async fn host_save(state: State<'_, AppState>, input: HostInput) -> ApiResult<HostView> {
    Ok(state.hosts.save(input).await?)
}

#[tauri::command]
pub async fn host_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.hosts.delete(id).await?)
}

#[tauri::command]
pub async fn import_ssh_config(state: State<'_, AppState>) -> ApiResult<usize> {
    Ok(state.import.import_ssh_config().await?)
}

#[tauri::command]
pub async fn identities_list(state: State<'_, AppState>) -> ApiResult<Vec<IdentityView>> {
    Ok(state.identities.list().await?)
}

#[tauri::command]
pub async fn identity_save(state: State<'_, AppState>, input: IdentityInput) -> ApiResult<IdentityView> {
    Ok(state.identities.save(input).await?)
}

#[tauri::command]
pub async fn identity_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.identities.delete(id).await?)
}

#[tauri::command]
pub async fn keys_list(state: State<'_, AppState>) -> ApiResult<Vec<KeyView>> {
    Ok(state.keys.list().await?)
}

#[tauri::command]
pub async fn key_import(state: State<'_, AppState>, input: KeyImportInput) -> ApiResult<KeyView> {
    Ok(state.keys.import(input).await?)
}

#[tauri::command]
pub async fn key_generate(state: State<'_, AppState>, input: KeyGenerateInput) -> ApiResult<KeyView> {
    Ok(state.keys.generate(input).await?)
}

#[tauri::command]
pub async fn key_rename(state: State<'_, AppState>, id: Id, label: String) -> ApiResult<KeyView> {
    Ok(state.keys.rename(id, &label).await?)
}

#[tauri::command]
pub async fn key_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.keys.delete(id).await?)
}

#[tauri::command]
pub async fn snippets_list(state: State<'_, AppState>) -> ApiResult<Vec<SnippetView>> {
    Ok(state.snippets.list().await?)
}

#[tauri::command]
pub async fn snippet_save(state: State<'_, AppState>, input: SnippetInput) -> ApiResult<SnippetView> {
    Ok(state.snippets.save(input).await?)
}

#[tauri::command]
pub async fn snippet_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.snippets.delete(id).await?)
}

#[tauri::command]
pub async fn known_hosts_list(state: State<'_, AppState>) -> ApiResult<Vec<KnownHostView>> {
    Ok(state.known_hosts.list().await?)
}

#[tauri::command]
pub async fn known_host_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.known_hosts.delete(id).await?)
}

#[tauri::command]
pub async fn forwards_list(state: State<'_, AppState>) -> ApiResult<Vec<PortForwardView>> {
    Ok(state.forwards.list().await?)
}

#[tauri::command]
pub async fn forward_save(state: State<'_, AppState>, input: PortForwardInput) -> ApiResult<PortForwardView> {
    Ok(state.forwards.save(input).await?)
}

#[tauri::command]
pub async fn forward_delete(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.forwards.delete(id).await?)
}
