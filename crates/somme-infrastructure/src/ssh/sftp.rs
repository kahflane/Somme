use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::Metadata;
use russh_sftp::protocol::StatusCode;
use somme_application::ports::{GatewayError, RemoteEntry, RemoteFs, RemoteHit};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt};

use crate::ssh::connect::Connection;
use crate::ssh::{connection_error, search, sudo};

fn transfer(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Transfer(err.to_string())
}

/// Like [`transfer`], but tells a refused file apart, so the user can be
/// offered sudo.
fn file_error(err: SftpError) -> GatewayError {
    match &err {
        SftpError::Status(status) if status.status_code == StatusCode::PermissionDenied => GatewayError::PermissionDenied,
        _ => transfer(err),
    }
}

// Entry names come from the server. A hostile server could send "../x" or a
// whole path, which would escape the folder once joined, so such names are dropped.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn entry(path: &str, meta: &Metadata) -> RemoteEntry {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    RemoteEntry {
        name: name.to_owned(),
        path: path.to_owned(),
        is_dir: meta.is_dir(),
        is_symlink: meta.is_symlink(),
        size: meta.size.unwrap_or(0),
        modified_at: meta.mtime.map(|t| i64::from(t) * 1000),
        permissions: meta.permissions.map(|_| meta.permissions().to_string()),
    }
}

pub struct SftpFs {
    conn: Arc<Connection>,
    sftp: SftpSession,
    /// Closing ends the connection too, unless a terminal shares it.
    owns_connection: bool,
}

impl SftpFs {
    pub async fn start(conn: Connection) -> Result<Self, GatewayError> {
        Self::open(Arc::new(conn), true).await
    }

    /// File access beside a terminal, on its connection.
    pub async fn beside(conn: Arc<Connection>) -> Result<Self, GatewayError> {
        Self::open(conn, false).await
    }

    async fn open(conn: Arc<Connection>, owns_connection: bool) -> Result<Self, GatewayError> {
        let channel = conn
            .target()
            .channel_open_session()
            .await
            .map_err(connection_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(connection_error)?;
        let sftp = SftpSession::new(channel.into_stream()).await.map_err(transfer)?;
        Ok(Self {
            conn,
            sftp,
            owns_connection,
        })
    }

    fn remove_tree<'a>(&'a self, path: &'a str) -> Pin<Box<dyn Future<Output = Result<(), GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            for entry in self.list(path).await? {
                if entry.is_dir && !entry.is_symlink {
                    self.remove_tree(&entry.path).await?;
                } else {
                    self.sftp.remove_file(entry.path).await.map_err(transfer)?;
                }
            }
            self.sftp.remove_dir(path).await.map_err(transfer)
        })
    }
}

#[async_trait]
impl RemoteFs for SftpFs {
    async fn home(&self) -> Result<String, GatewayError> {
        self.sftp.canonicalize(".").await.map_err(transfer)
    }

    async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, GatewayError> {
        let entries = self.sftp.read_dir(path).await.map_err(transfer)?;
        Ok(entries
            .filter(|e| is_plain_name(&e.file_name()))
            .map(|e| {
                let meta = e.metadata();
                let file_type = e.file_type();
                let name = e.file_name();
                RemoteEntry {
                    path: join(path, &name),
                    name,
                    is_dir: file_type.is_dir(),
                    is_symlink: file_type.is_symlink(),
                    size: meta.size.unwrap_or(0),
                    modified_at: meta.mtime.map(|t| i64::from(t) * 1000),
                    permissions: meta.permissions.map(|_| meta.permissions().to_string()),
                }
            })
            .collect())
    }

    async fn mkdir(&self, path: &str) -> Result<(), GatewayError> {
        self.sftp.create_dir(path).await.map_err(transfer)
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), GatewayError> {
        self.sftp.rename(from, to).await.map_err(transfer)
    }

    async fn remove(&self, path: &str, is_dir: bool) -> Result<(), GatewayError> {
        if is_dir {
            self.remove_tree(path).await
        } else {
            self.sftp.remove_file(path).await.map_err(transfer)
        }
    }

    async fn download(&self, remote: &str, local: &Path) -> Result<(), GatewayError> {
        let mut source = self.sftp.open(remote).await.map_err(transfer)?;
        let mut target = tokio::fs::File::create(local).await.map_err(transfer)?;
        tokio::io::copy(&mut source, &mut target).await.map_err(transfer)?;
        target.flush().await.map_err(transfer)
    }

    async fn upload(&self, local: &Path, remote: &str) -> Result<(), GatewayError> {
        let mut source = tokio::fs::File::open(local).await.map_err(transfer)?;
        let mut target = self.sftp.create(remote).await.map_err(transfer)?;
        tokio::io::copy(&mut source, &mut target).await.map_err(transfer)?;
        target.shutdown().await.map_err(transfer)
    }

    async fn stat(&self, path: &str) -> Result<RemoteEntry, GatewayError> {
        let meta = self.sftp.metadata(path).await.map_err(file_error)?;
        Ok(entry(path, &meta))
    }

    async fn read(&self, path: &str, limit: u64) -> Result<Vec<u8>, GatewayError> {
        let size = self.sftp.metadata(path).await.map_err(file_error)?.size.unwrap_or(0);
        if size > limit {
            return Err(GatewayError::Transfer(format!("the file is over {} MB", limit / (1024 * 1024))));
        }
        let mut file = self.sftp.open(path).await.map_err(file_error)?;
        let mut bytes = Vec::new();
        // The file may grow while it is read.
        (&mut file).take(limit + 1).read_to_end(&mut bytes).await.map_err(transfer)?;
        let _ = file.shutdown().await;
        if bytes.len() as u64 > limit {
            return Err(GatewayError::Transfer(format!("the file is over {} MB", limit / (1024 * 1024))));
        }
        Ok(bytes)
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), GatewayError> {
        let mut file = self.sftp.create(path).await.map_err(file_error)?;
        file.write_all(data).await.map_err(transfer)?;
        file.shutdown().await.map_err(transfer)
    }

    async fn read_as_root(&self, path: &str, limit: u64, password: Option<&str>) -> Result<Vec<u8>, GatewayError> {
        sudo::read(&self.conn, path, limit, password).await
    }

    async fn write_as_root(&self, path: &str, data: &[u8], password: Option<&str>) -> Result<(), GatewayError> {
        sudo::write(&self.conn, path, data, password).await
    }

    async fn search(
        &self,
        dir: &str,
        query: &str,
        text: bool,
        ignore_case: bool,
        limit: usize,
    ) -> Result<Vec<RemoteHit>, GatewayError> {
        search::search(&self.conn, dir, query, text, ignore_case, limit).await
    }

    async fn close(&self) {
        let _ = self.sftp.close().await;
        if self.owns_connection {
            self.conn.disconnect().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_plain_name;

    #[test]
    fn accepts_normal_names() {
        for name in ["notes.txt", ".bashrc", "backup 12:00.tar", "..hidden"] {
            assert!(is_plain_name(name), "{name}");
        }
    }

    #[test]
    fn rejects_names_that_escape_the_folder() {
        for name in ["", ".", "..", "../x", "a/b", "/etc/passwd", "..\\x", "a\\b", "a\0b"] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }
}
