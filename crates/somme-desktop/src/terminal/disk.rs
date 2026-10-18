//! Folders and files where a shell runs: on this computer, or on the server an
//! SSH shell runs on. Every call blocks, so [`Disk::run`] runs them off the UI
//! thread.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use somme_application::ports::{GatewayError, RemoteFs};

use crate::runtime;

#[derive(Clone)]
pub enum Disk {
    Local,
    /// A server's files. `home` is the user's home folder there, once the shell said.
    Remote { files: Arc<dyn RemoteFs>, home: Option<PathBuf> },
}

/// Why a file could not be read or saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskError {
    /// The user may not read or change it. On a server, sudo may still.
    Denied,
    /// sudo wants the user's password, or did not take the one given.
    Password { wrong: bool },
    /// The user stopped, so there is nothing to tell them.
    Cancelled,
    Other(String),
}

impl std::fmt::Display for DiskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => f.write_str("You don't have permission."),
            Self::Password { wrong: false } => f.write_str("sudo needs your password."),
            Self::Password { wrong: true } => f.write_str("sudo did not accept the password."),
            Self::Cancelled => f.write_str("Cancelled."),
            Self::Other(reason) => f.write_str(reason),
        }
    }
}

impl From<GatewayError> for DiskError {
    fn from(err: GatewayError) -> Self {
        match err {
            GatewayError::PermissionDenied => Self::Denied,
            GatewayError::SudoNeedsPassword => Self::Password { wrong: false },
            GatewayError::SudoWrongPassword => Self::Password { wrong: true },
            GatewayError::Cancelled => Self::Cancelled,
            // Already says what happened, without a prefix.
            GatewayError::Session(reason) => Self::Other(reason),
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<std::io::Error> for DiskError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::PermissionDenied => Self::Denied,
            _ => Self::Other(err.to_string()),
        }
    }
}

/// A name in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Follows links, so a link to a folder counts as a folder.
    pub dir: bool,
    pub runnable: bool,
}

impl Disk {
    pub fn is_local(&self) -> bool {
        matches!(self, Disk::Local)
    }

    /// Whether files refused to the user can be tried again with sudo.
    pub fn can_sudo(&self) -> bool {
        !self.is_local()
    }

    /// Whether both reach the same files.
    pub fn same(&self, other: &Disk) -> bool {
        match (self, other) {
            (Disk::Local, Disk::Local) => true,
            (Disk::Remote { files: a, .. }, Disk::Remote { files: b, .. }) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn home(&self) -> Option<PathBuf> {
        match self {
            Disk::Local => dirs::home_dir(),
            Disk::Remote { home, .. } => home.clone(),
        }
    }

    /// Whether `path` is a folder, or None when nothing is there. Follows links.
    pub fn is_dir(&self, path: &Path) -> Option<bool> {
        match self {
            Disk::Local => std::fs::metadata(path).ok().map(|meta| meta.is_dir()),
            Disk::Remote { files, .. } => block(files.stat(&remote(path))).ok().map(|entry| entry.is_dir),
        }
    }

    /// Up to `limit` names in `dir` that pass `keep`. None when the folder can't be read.
    pub fn list(&self, dir: &Path, limit: usize, keep: impl Fn(&str) -> bool) -> Option<Vec<Entry>> {
        match self {
            Disk::Local => {
                let entries = std::fs::read_dir(dir).ok()?;
                Some(
                    entries
                        .flatten()
                        .filter_map(|entry| entry.file_name().into_string().ok().map(|name| (name, entry.path())))
                        .filter(|(name, _)| keep(name))
                        .take(limit)
                        .map(|(name, path)| {
                            let meta = std::fs::metadata(path).ok();
                            Entry {
                                name,
                                dir: meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
                                runnable: meta.as_ref().is_some_and(is_runnable),
                            }
                        })
                        .collect(),
                )
            }
            Disk::Remote { files, .. } => {
                let entries = block(files.list(&remote(dir))).ok()?;
                Some(
                    entries
                        .into_iter()
                        .filter(|entry| keep(&entry.name))
                        .take(limit)
                        .map(|entry| {
                            // Only links need a second look, to see where they go.
                            let dir = if entry.is_symlink {
                                block(files.stat(&entry.path)).is_ok_and(|target| target.is_dir)
                            } else {
                                entry.is_dir
                            };
                            Entry {
                                runnable: entry.permissions.as_deref().is_some_and(|mode| mode.contains('x')),
                                name: entry.name,
                                dir,
                            }
                        })
                        .collect(),
                )
            }
        }
    }

    /// A file's bytes. Files over `limit` bytes are refused.
    pub fn read(&self, path: &Path, limit: u64) -> Result<Vec<u8>, DiskError> {
        match self {
            Disk::Local => {
                let size = std::fs::metadata(path)?.len();
                if size > limit {
                    return Err(DiskError::Other(too_big(size, limit)));
                }
                Ok(std::fs::read(path)?)
            }
            Disk::Remote { files, .. } => Ok(block(files.read(&remote(path), limit))?),
        }
    }

    /// Copies a file to `local`, on this computer.
    pub fn download(&self, path: &Path, local: &Path) -> Result<(), DiskError> {
        match self {
            Disk::Local => Ok(std::fs::copy(path, local).map(drop)?),
            Disk::Remote { files, .. } => Ok(block(files.download(&remote(path), local))?),
        }
    }

    pub fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), DiskError> {
        match self {
            Disk::Local => Ok(std::fs::write(path, bytes)?),
            Disk::Remote { files, .. } => Ok(block(files.write(&remote(path), bytes))?),
        }
    }

    /// [`Disk::read`] as root, with sudo. `password` is for when sudo asks.
    pub fn read_as_root(&self, path: &Path, limit: u64, password: Option<&str>) -> Result<Vec<u8>, DiskError> {
        match self {
            Disk::Local => Err(no_local_sudo()),
            Disk::Remote { files, .. } => Ok(block(files.read_as_root(&remote(path), limit, password))?),
        }
    }

    /// [`Disk::write`] as root, with sudo. `password` is for when sudo asks.
    pub fn write_as_root(&self, path: &Path, bytes: &[u8], password: Option<&str>) -> Result<(), DiskError> {
        match self {
            Disk::Local => Err(no_local_sudo()),
            Disk::Remote { files, .. } => Ok(block(files.write_as_root(&remote(path), bytes, password))?),
        }
    }

    /// Runs `work` with this disk on a thread that may block.
    pub fn run<T: Send + 'static>(self, work: impl FnOnce(&Disk) -> T + Send + 'static) -> impl Future<Output = T> {
        runtime::io(async move {
            match tokio::task::spawn_blocking(move || work(&self)).await {
                Ok(value) => value,
                Err(err) => std::panic::resume_unwind(err.into_panic()),
            }
        })
    }
}

pub fn too_big(size: u64, limit: u64) -> String {
    format!("It is {} MB. Files up to {} MB open here.", size / 1_048_576, limit / 1_048_576)
}

fn no_local_sudo() -> DiskError {
    DiskError::Other("Somme uses sudo only for files on servers.".into())
}

/// A server path as the server writes it.
fn remote(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Waits for a server call. Only for threads from [`Disk::run`].
fn block<F: Future>(future: F) -> F::Output {
    runtime::handle().block_on(future)
}

#[cfg(unix)]
fn is_runnable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_runnable(meta: &std::fs::Metadata) -> bool {
    meta.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_file_says_so() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("locked.txt");
        std::fs::write(&file, "secret").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(Disk::Local.read(&file, 1024), Err(DiskError::Denied));
        assert_eq!(Disk::Local.write(&file, b"x"), Err(DiskError::Denied));
        assert!(matches!(Disk::Local.read(&dir.path().join("missing"), 1024), Err(DiskError::Other(_))));
        assert_eq!(DiskError::from(GatewayError::SudoWrongPassword), DiskError::Password { wrong: true });
        assert_eq!(DiskError::from(GatewayError::Session("no sudo".into())), DiskError::Other("no sudo".into()));
    }
}

/// A server for tests, with its files in a folder on this computer. Paths
/// work the way they do on a server, with `/`.
#[cfg(test)]
pub mod fake {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use async_trait::async_trait;
    use somme_application::ports::{GatewayError, RemoteEntry, RemoteFs};

    use super::Disk;

    pub const HOME: &str = "/home/me";

    pub struct Server {
        root: PathBuf,
    }

    /// A server whose `/` is `root`, logged in as a user with [`HOME`].
    pub fn disk(root: &Path) -> Disk {
        crate::runtime::init().unwrap();
        Disk::Remote {
            files: Arc::new(Server { root: root.to_path_buf() }),
            home: Some(PathBuf::from(HOME)),
        }
    }

    impl Server {
        fn local(&self, path: &str) -> PathBuf {
            assert!(!path.contains('\\'), "server paths use /: {path}");
            self.root.join(path.trim_start_matches('/'))
        }
    }

    fn error(err: std::io::Error) -> GatewayError {
        match err.kind() {
            std::io::ErrorKind::PermissionDenied => GatewayError::PermissionDenied,
            _ => GatewayError::Transfer(err.to_string()),
        }
    }

    fn entry(path: &str, meta: &std::fs::Metadata, is_symlink: bool) -> RemoteEntry {
        RemoteEntry {
            name: path.rsplit('/').next().unwrap_or_default().to_owned(),
            path: path.to_owned(),
            is_dir: meta.is_dir(),
            is_symlink,
            size: meta.len(),
            modified_at: None,
            permissions: None,
        }
    }

    #[async_trait]
    impl RemoteFs for Server {
        async fn home(&self) -> Result<String, GatewayError> {
            Ok(HOME.into())
        }

        async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, GatewayError> {
            let mut entries = Vec::new();
            for found in std::fs::read_dir(self.local(path)).map_err(error)? {
                let found = found.map_err(error)?;
                // Like SFTP, a link shows as a link, not as what it points at.
                let meta = std::fs::symlink_metadata(found.path()).map_err(error)?;
                let name = found.file_name().to_string_lossy().into_owned();
                entries.push(entry(&format!("{}/{name}", path.trim_end_matches('/')), &meta, meta.is_symlink()));
            }
            Ok(entries)
        }

        async fn mkdir(&self, path: &str) -> Result<(), GatewayError> {
            std::fs::create_dir(self.local(path)).map_err(error)
        }

        async fn rename(&self, from: &str, to: &str) -> Result<(), GatewayError> {
            std::fs::rename(self.local(from), self.local(to)).map_err(error)
        }

        async fn remove(&self, path: &str, is_dir: bool) -> Result<(), GatewayError> {
            if is_dir {
                std::fs::remove_dir_all(self.local(path)).map_err(error)
            } else {
                std::fs::remove_file(self.local(path)).map_err(error)
            }
        }

        async fn download(&self, remote: &str, local: &Path) -> Result<(), GatewayError> {
            std::fs::copy(self.local(remote), local).map(drop).map_err(error)
        }

        async fn upload(&self, local: &Path, remote: &str) -> Result<(), GatewayError> {
            std::fs::copy(local, self.local(remote)).map(drop).map_err(error)
        }

        async fn stat(&self, path: &str) -> Result<RemoteEntry, GatewayError> {
            let meta = std::fs::metadata(self.local(path)).map_err(error)?;
            Ok(entry(path, &meta, false))
        }

        async fn read(&self, path: &str, limit: u64) -> Result<Vec<u8>, GatewayError> {
            let bytes = std::fs::read(self.local(path)).map_err(error)?;
            if bytes.len() as u64 > limit {
                return Err(GatewayError::Transfer("too big".into()));
            }
            Ok(bytes)
        }

        async fn write(&self, path: &str, data: &[u8]) -> Result<(), GatewayError> {
            std::fs::write(self.local(path), data).map_err(error)
        }

        async fn close(&self) {}
    }
}
