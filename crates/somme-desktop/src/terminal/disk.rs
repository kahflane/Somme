//! Folders and files where a shell runs: on this computer, or on the server an
//! SSH shell runs on. Every call blocks, so [`Disk::run`] runs them off the UI
//! thread.

use std::future::Future;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use somme_application::ports::{GatewayError, RemoteFs, RemoteHit};

use crate::runtime;

/// How often a server search looks whether it should stop.
const STOP_CHECK: Duration = Duration::from_millis(100);
/// Deeper folders are almost always a link that loops.
const MAX_DEPTH: usize = 32;

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
                // Something with no size, like a device, may never end.
                let mut bytes = Vec::new();
                std::fs::File::open(path)?.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > limit {
                    return Err(DiskError::Other(too_big(bytes.len() as u64, limit)));
                }
                Ok(bytes)
            }
            Disk::Remote { files, .. } => Ok(block(files.read(&remote(path), limit))?),
        }
    }

    /// A file's size. Folders are refused, and on this computer so is anything
    /// that is not a plain file, like a device or a pipe, as reading it may
    /// never end. A server doesn't tell those apart, so read them with a limit.
    pub fn size(&self, path: &Path) -> Result<u64, DiskError> {
        match self {
            Disk::Local => {
                let meta = std::fs::metadata(path)?;
                if !meta.is_file() {
                    return Err(DiskError::Other("It is not a regular file.".into()));
                }
                Ok(meta.len())
            }
            Disk::Remote { files, .. } => {
                let entry = block(files.stat(&remote(path)))?;
                if entry.is_dir {
                    return Err(DiskError::Other("It is a folder.".into()));
                }
                Ok(entry.size)
            }
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

    /// Looks for files on a server, as [`crate::search`] does here. Setting
    /// `stop` ends the search there too.
    pub fn server_search(
        &self,
        dir: &Path,
        query: &str,
        text: bool,
        ignore_case: bool,
        limit: usize,
        stop: &AtomicBool,
    ) -> Result<Vec<RemoteHit>, DiskError> {
        let Disk::Remote { files, .. } = self else {
            return Err(DiskError::Other("This is not a server.".into()));
        };
        let dir = remote(dir);
        block(async {
            let stopped = async {
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(STOP_CHECK).await;
                }
            };
            tokio::select! {
                hits = files.search(&dir, query, text, ignore_case, limit) => Ok(hits?),
                () = stopped => Ok(Vec::new()),
            }
        })
    }

    /// The home folder, asking the server when the shell has not said.
    pub fn find_home(&self) -> Option<PathBuf> {
        match self {
            Disk::Remote { files, home: None } => block(files.home()).ok().map(PathBuf::from),
            _ => self.home(),
        }
    }

    /// Copies a file or folder on this computer into the server folder `dir`,
    /// with all that is in it. Files with the same name are replaced. Gives
    /// how many files were copied.
    pub fn upload(&self, local: &Path, dir: &Path) -> Result<u64, DiskError> {
        let Disk::Remote { files, .. } = self else {
            return Err(DiskError::Other("This is not a server.".into()));
        };
        upload(files.as_ref(), local, &remote(dir), 0)
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

fn upload(files: &dyn RemoteFs, local: &Path, dir: &str, depth: usize) -> Result<u64, DiskError> {
    let name = local.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(DiskError::Other(format!("{} has a name the server can't take.", local.display())));
    }
    let target = if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
    // Follows links, as the copy should hold what they point at.
    let meta = std::fs::metadata(local)?;
    if meta.is_file() {
        block(files.upload(local, &target))?;
        return Ok(1);
    }
    // Pipes and devices may never end.
    if !meta.is_dir() {
        return Ok(0);
    }
    if depth >= MAX_DEPTH {
        return Err(DiskError::Other(format!("{} is nested too deep.", local.display())));
    }
    // The folder may be there already. A real problem shows up on the first file.
    let _ = block(files.mkdir(&target));
    let mut count = 0;
    for entry in std::fs::read_dir(local)? {
        count += upload(files, &entry?.path(), &target, depth + 1)?;
    }
    Ok(count)
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

    // Windows has no file modes to take away.
    #[cfg(unix)]
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

    #[test]
    fn a_folder_goes_up_with_all_inside() {
        let (here, server) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let project = here.path().join("my project");
        std::fs::create_dir_all(project.join("src/deep")).unwrap();
        std::fs::write(project.join("README.md"), "hi").unwrap();
        std::fs::write(project.join("src/deep/main.rs"), "fn main() {}").unwrap();
        std::fs::write(here.path().join("notes.txt"), "notes").unwrap();
        std::fs::create_dir_all(server.path().join("home/me")).unwrap();
        let disk = fake::disk(server.path());
        let home = Path::new(fake::HOME);
        assert_eq!(disk.upload(&project, home), Ok(2));
        assert_eq!(disk.upload(&here.path().join("notes.txt"), home), Ok(1));
        // Again, over what is there now.
        assert_eq!(disk.upload(&project, home), Ok(2));
        let up = server.path().join("home/me");
        assert_eq!(std::fs::read_to_string(up.join("my project/src/deep/main.rs")).unwrap(), "fn main() {}");
        assert_eq!(std::fs::read_to_string(up.join("notes.txt")).unwrap(), "notes");
        assert!(Disk::Local.upload(&project, home).is_err());
    }
}

/// A server for tests, with its files in a folder on this computer. Paths
/// work the way they do on a server, with `/`.
#[cfg(test)]
pub mod fake {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use async_trait::async_trait;
    use somme_application::ports::{GatewayError, RemoteEntry, RemoteFs, RemoteHit};

    use super::Disk;
    use crate::search::{self, Find, Hit};

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

        async fn search(
            &self,
            dir: &str,
            query: &str,
            text: bool,
            _: bool,
            limit: usize,
        ) -> Result<Vec<RemoteHit>, GatewayError> {
            let found = std::sync::Mutex::new(Vec::new());
            let find = if text { Find::Text } else { Find::Names };
            let stop = std::sync::atomic::AtomicBool::new(false);
            search::walk(&[self.local(dir)], find, query, &stop, &|hit: Hit| found.lock().unwrap().push(hit));
            let hits = found.into_inner().unwrap().into_iter().take(limit);
            Ok(hits
                .map(|hit| RemoteHit {
                    path: format!("/{}", hit.path.strip_prefix(&self.root).unwrap().to_string_lossy()),
                    is_dir: hit.folder,
                    line: hit.line,
                })
                .collect())
        }

        async fn close(&self) {}
    }
}
