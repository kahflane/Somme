//! File operations behind the SFTP screen. The local disk and a remote host
//! look the same from here, so copies work in any direction.

use std::fs::Metadata;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::UNIX_EPOCH;

use somme_application::dto::SftpEntryView;
use somme_domain::{Id, new_id};

use crate::services::Services;

pub type FsResult<T> = Result<T, String>;

/// Deeper folders are almost always a symlink loop.
const MAX_DEPTH: usize = 32;

/// Where a pane's files live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Local,
    /// An open SFTP session.
    Remote(Id),
}

fn text(err: impl std::fmt::Display) -> String {
    err.to_string()
}

impl Endpoint {
    pub fn join(self, dir: &str, name: &str) -> String {
        match self {
            Endpoint::Local => Path::new(dir).join(name).to_string_lossy().into_owned(),
            Endpoint::Remote(_) if dir.ends_with('/') => format!("{dir}{name}"),
            Endpoint::Remote(_) => format!("{dir}/{name}"),
        }
    }

    /// The folder above `path`, or `None` at the root.
    pub fn parent(self, path: &str) -> Option<String> {
        match self {
            Endpoint::Local => Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()),
            Endpoint::Remote(_) => {
                let trimmed = path.trim_end_matches('/');
                match trimmed.rfind('/') {
                    _ if trimmed.is_empty() => None,
                    Some(0) => Some("/".to_owned()),
                    Some(ix) => Some(trimmed[..ix].to_owned()),
                    None => None,
                }
            }
        }
    }

    pub async fn list(self, services: &Services, path: &str) -> FsResult<Vec<SftpEntryView>> {
        match self {
            Endpoint::Local => local_list(path).await.map_err(text),
            Endpoint::Remote(id) => services.sftp.list(id, path).await.map_err(text),
        }
    }

    pub async fn mkdir(self, services: &Services, path: &str) -> FsResult<()> {
        match self {
            Endpoint::Local => tokio::fs::create_dir(path).await.map_err(text),
            Endpoint::Remote(id) => services.sftp.mkdir(id, path).await.map_err(text),
        }
    }

    pub async fn rename(self, services: &Services, from: &str, to: &str) -> FsResult<()> {
        match self {
            Endpoint::Local => tokio::fs::rename(from, to).await.map_err(text),
            Endpoint::Remote(id) => services.sftp.rename(id, from, to).await.map_err(text),
        }
    }

    pub async fn remove(self, services: &Services, entry: &SftpEntryView) -> FsResult<()> {
        // A link to a folder is removed as a link, never by emptying the folder it points to.
        let is_dir = entry.is_dir && !entry.is_symlink;
        match self {
            Endpoint::Local if is_dir => tokio::fs::remove_dir_all(&entry.path).await.map_err(text),
            Endpoint::Local => tokio::fs::remove_file(&entry.path).await.map_err(text),
            Endpoint::Remote(id) => services.sftp.remove(id, &entry.path, is_dir).await.map_err(text),
        }
    }
}

/// Copies a file or folder into `dest_dir`. Existing files with the same name are replaced.
/// Returns how many files were copied.
pub async fn copy(
    services: &Services,
    from: Endpoint,
    entry: &SftpEntryView,
    to: Endpoint,
    dest_dir: &str,
) -> FsResult<u64> {
    let target = to.join(dest_dir, &entry.name);
    if from == to {
        if target == entry.path {
            return Err("It is already in this folder.".into());
        }
        if entry.is_dir && target.starts_with(&format!("{}/", entry.path.trim_end_matches('/'))) {
            return Err("A folder cannot be copied into itself.".into());
        }
    }
    copy_entry(services, from, entry, to, dest_dir, 0).await
}

fn copy_entry<'a>(
    services: &'a Services,
    from: Endpoint,
    entry: &'a SftpEntryView,
    to: Endpoint,
    dest_dir: &'a str,
    depth: usize,
) -> Pin<Box<dyn Future<Output = FsResult<u64>> + Send + 'a>> {
    Box::pin(async move {
        // The SFTP listing already drops these, but a name like "../x" must never reach a join.
        if !is_plain_name(&entry.name) {
            return Err(format!("Skipped {:?}: not a plain file name.", entry.name));
        }
        if !entry.is_dir {
            copy_file(services, from, entry, to, dest_dir).await?;
            return Ok(1);
        }
        if depth >= MAX_DEPTH {
            return Err(format!("{} is nested too deep.", entry.path));
        }
        let target = to.join(dest_dir, &entry.name);
        // The folder may already be there from an earlier copy. A real problem shows up on the first file.
        let _ = to.mkdir(services, &target).await;
        let mut count = 0;
        for child in from.list(services, &entry.path).await? {
            count += copy_entry(services, from, &child, to, &target, depth + 1).await?;
        }
        Ok(count)
    })
}

async fn copy_file(
    services: &Services,
    from: Endpoint,
    entry: &SftpEntryView,
    to: Endpoint,
    dest_dir: &str,
) -> FsResult<()> {
    let local_target = || Path::new(dest_dir).join(&entry.name);
    match (from, to) {
        (Endpoint::Local, Endpoint::Local) => tokio::fs::copy(&entry.path, local_target())
            .await
            .map(|_| ())
            .map_err(text),
        (Endpoint::Local, Endpoint::Remote(id)) => services
            .sftp
            .upload(id, Path::new(&entry.path), dest_dir)
            .await
            .map(|_| ())
            .map_err(text),
        (Endpoint::Remote(id), Endpoint::Local) => services
            .sftp
            .download(id, &entry.path, &local_target())
            .await
            .map_err(text),
        (Endpoint::Remote(source), Endpoint::Remote(target)) => {
            // The two servers cannot reach each other, so the file passes through this computer.
            let dir = std::env::temp_dir().join(format!("somme-transfer-{}", new_id()));
            tokio::fs::create_dir_all(&dir).await.map_err(text)?;
            let local = dir.join(&entry.name);
            let result = async {
                services.sftp.download(source, &entry.path, &local).await.map_err(text)?;
                services.sftp.upload(target, &local, dest_dir).await.map(|_| ()).map_err(text)
            }
            .await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            result
        }
    }
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

pub fn local_home() -> String {
    dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_owned())
}

/// Describes a local file, for files dropped in from Finder.
pub fn local_entry(path: &Path) -> Option<SftpEntryView> {
    let link = std::fs::symlink_metadata(path).ok()?;
    let is_symlink = link.file_type().is_symlink();
    let meta = if is_symlink { std::fs::metadata(path).unwrap_or(link) } else { link };
    Some(entry_view(path, &meta, is_symlink))
}

async fn local_list(path: &str) -> std::io::Result<Vec<SftpEntryView>> {
    let mut reader = tokio::fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let link = entry.metadata().await?;
        let is_symlink = link.file_type().is_symlink();
        let meta = if is_symlink {
            tokio::fs::metadata(entry.path()).await.unwrap_or(link)
        } else {
            link
        };
        entries.push(entry_view(&entry.path(), &meta, is_symlink));
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn entry_view(path: &Path, meta: &Metadata, is_symlink: bool) -> SftpEntryView {
    SftpEntryView {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        path: path.to_string_lossy().into_owned(),
        is_dir: meta.is_dir(),
        is_symlink,
        size: meta.len(),
        modified_at: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok()),
        permissions: permissions(meta, is_symlink),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_parent() {
        let remote = Endpoint::Remote(new_id());
        assert_eq!(remote.parent("/home/a/"), Some("/home".into()));
        assert_eq!(remote.parent("/home"), Some("/".into()));
        assert_eq!(remote.parent("/"), None);
        assert_eq!(remote.join("/", "x"), "/x");
        assert_eq!(remote.join("/home", "x"), "/home/x");
    }

    #[test]
    fn plain_names() {
        for name in ["notes.txt", ".bashrc", "..hidden"] {
            assert!(is_plain_name(name), "{name}");
        }
        for name in ["", ".", "..", "../x", "a/b", "/etc/passwd", "a\\b", "a\0b"] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }
}
