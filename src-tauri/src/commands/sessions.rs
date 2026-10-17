use std::path::PathBuf;
use std::sync::Arc;

use somme_application::dto::{PromptResponse, SftpEntryView, SftpOpened};
use somme_application::ports::{OutputSink, TermSize};
use somme_domain::Id;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, State};

use crate::error::ApiResult;
use crate::events::ChannelSink;
use crate::state::AppState;

fn sink(app: AppHandle, channel: Channel<InvokeResponseBody>) -> impl FnOnce(Id) -> Arc<dyn OutputSink> {
    move |id| Arc::new(ChannelSink::new(app, id, channel))
}

#[tauri::command]
pub async fn ssh_open(
    app: AppHandle,
    state: State<'_, AppState>,
    host_id: Id,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> ApiResult<Id> {
    let size = TermSize { cols, rows };
    Ok(state.sessions.open_ssh(host_id, size, sink(app, on_data)).await?)
}

#[tauri::command]
pub fn local_open(
    app: AppHandle,
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> ApiResult<Id> {
    let size = TermSize { cols, rows };
    Ok(state.sessions.open_local(size, sink(app, on_data))?)
}

#[tauri::command]
pub async fn session_write(state: State<'_, AppState>, session_id: Id, data: String) -> ApiResult<()> {
    Ok(state.sessions.write(session_id, data.as_bytes()).await?)
}

#[tauri::command]
pub async fn session_resize(state: State<'_, AppState>, session_id: Id, cols: u16, rows: u16) -> ApiResult<()> {
    Ok(state.sessions.resize(session_id, TermSize { cols, rows }).await?)
}

#[tauri::command]
pub async fn session_close(state: State<'_, AppState>, session_id: Id) -> ApiResult<()> {
    state.sessions.close(session_id).await;
    Ok(())
}

#[tauri::command]
pub fn prompt_respond(state: State<'_, AppState>, id: Id, response: PromptResponse) {
    state.prompter.respond(id, response.into());
}

#[tauri::command]
pub async fn forward_start(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    Ok(state.forwards.start(id).await?)
}

#[tauri::command]
pub async fn forward_stop(state: State<'_, AppState>, id: Id) -> ApiResult<()> {
    state.forwards.stop(id).await;
    Ok(())
}

#[tauri::command]
pub fn forwards_active(state: State<'_, AppState>) -> Vec<Id> {
    state.forwards.active()
}

#[tauri::command]
pub async fn sftp_open(state: State<'_, AppState>, host_id: Id) -> ApiResult<SftpOpened> {
    Ok(state.sftp.open(host_id).await?)
}

#[tauri::command]
pub async fn sftp_list(state: State<'_, AppState>, sftp_id: Id, path: String) -> ApiResult<Vec<SftpEntryView>> {
    Ok(state.sftp.list(sftp_id, &path).await?)
}

#[tauri::command]
pub async fn sftp_mkdir(state: State<'_, AppState>, sftp_id: Id, path: String) -> ApiResult<()> {
    Ok(state.sftp.mkdir(sftp_id, &path).await?)
}

#[tauri::command]
pub async fn sftp_rename(state: State<'_, AppState>, sftp_id: Id, from: String, to: String) -> ApiResult<()> {
    Ok(state.sftp.rename(sftp_id, &from, &to).await?)
}

#[tauri::command]
pub async fn sftp_remove(state: State<'_, AppState>, sftp_id: Id, path: String, is_dir: bool) -> ApiResult<()> {
    Ok(state.sftp.remove(sftp_id, &path, is_dir).await?)
}

#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    sftp_id: Id,
    remote_path: String,
    local_path: PathBuf,
) -> ApiResult<()> {
    Ok(state.sftp.download(sftp_id, &remote_path, &local_path).await?)
}

#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, AppState>,
    sftp_id: Id,
    local_path: PathBuf,
    remote_dir: String,
) -> ApiResult<String> {
    Ok(state.sftp.upload(sftp_id, &local_path, &remote_dir).await?)
}

#[tauri::command]
pub async fn sftp_close(state: State<'_, AppState>, sftp_id: Id) -> ApiResult<()> {
    state.sftp.close(sftp_id).await;
    Ok(())
}
