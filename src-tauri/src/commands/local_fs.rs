use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use somme_application::dto::SftpEntryView;

use crate::error::{ApiError, ApiResult};

#[tauri::command]
pub fn local_home() -> ApiResult<String> {
    dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .ok_or_else(|| ApiError::io("no home directory"))
}

#[tauri::command]
pub async fn local_list(path: String) -> ApiResult<Vec<SftpEntryView>> {
    let mut reader = tokio::fs::read_dir(&path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let link = entry.metadata().await?;
        let is_symlink = link.file_type().is_symlink();
        let meta = if is_symlink {
            tokio::fs::metadata(entry.path()).await.unwrap_or(link)
        } else {
            link
        };
        entries.push(SftpEntryView {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            is_symlink,
            size: meta.len(),
            modified_at: modified_millis(&meta),
            permissions: permissions(&meta, is_symlink),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[tauri::command]
pub async fn local_mkdir(path: String) -> ApiResult<()> {
    Ok(tokio::fs::create_dir(&path).await?)
}

#[tauri::command]
pub async fn local_rename(from: String, to: String) -> ApiResult<()> {
    if Path::new(&to).exists() {
        return Err(ApiError::io("a file with that name already exists"));
    }
    Ok(tokio::fs::rename(&from, &to).await?)
}

#[tauri::command]
pub async fn local_remove(path: String, is_dir: bool) -> ApiResult<()> {
    let path = PathBuf::from(path);
    if is_dir {
        Ok(tokio::fs::remove_dir_all(&path).await?)
    } else {
        Ok(tokio::fs::remove_file(&path).await?)
    }
}

fn modified_millis(meta: &Metadata) -> Option<i64> {
    let since = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

#[cfg(unix)]
fn permissions(meta: &Metadata, is_symlink: bool) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = meta.permissions().mode();
    let kind = if is_symlink {
        'l'
    } else if meta.is_dir() {
        'd'
    } else {
        '-'
    };
    let mut text = String::with_capacity(10);
    text.push(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        text.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        text.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    Some(text)
}

#[cfg(not(unix))]
fn permissions(_meta: &Metadata, _is_symlink: bool) -> Option<String> {
    None
}
