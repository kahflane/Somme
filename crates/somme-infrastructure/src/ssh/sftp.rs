use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use async_trait::async_trait;
use russh_sftp::client::SftpSession;
use somme_application::ports::{GatewayError, RemoteEntry, RemoteFs};
use tokio::io::AsyncWriteExt;

use crate::ssh::connect::Connection;
use crate::ssh::connection_error;

fn transfer(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Transfer(err.to_string())
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub struct SftpFs {
    conn: Connection,
    sftp: SftpSession,
}

impl SftpFs {
    pub async fn start(conn: Connection) -> Result<Self, GatewayError> {
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
        Ok(Self { conn, sftp })
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
            .filter(|e| {
                let name = e.file_name();
                name != "." && name != ".."
            })
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

    async fn close(&self) {
        let _ = self.sftp.close().await;
        self.conn.disconnect().await;
    }
}
