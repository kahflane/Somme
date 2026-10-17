use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use somme_domain::{DomainError, Id, new_id};

use crate::dto::{SftpEntryView, SftpOpened};
use crate::error::{AppError, AppResult};
use crate::ports::{ConnectContext, RemoteFs, SshGateway};
use crate::services::ConnectionPlanner;

pub struct SftpService {
    planner: Arc<ConnectionPlanner>,
    ssh: Arc<dyn SshGateway>,
    ctx: ConnectContext,
    sessions: DashMap<Id, Arc<dyn RemoteFs>>,
}

impl SftpService {
    pub fn new(planner: Arc<ConnectionPlanner>, ssh: Arc<dyn SshGateway>, ctx: ConnectContext) -> Self {
        Self {
            planner,
            ssh,
            ctx,
            sessions: DashMap::new(),
        }
    }

    pub async fn open(&self, host_id: Id) -> AppResult<SftpOpened> {
        let (_, plan) = self.planner.plan(host_id).await?;
        let fs = self.ssh.sftp(plan, self.ctx.clone()).await?;
        let home = fs.home().await?;
        let sftp_id = new_id();
        self.sessions.insert(sftp_id, fs);
        Ok(SftpOpened { sftp_id, home })
    }

    pub async fn list(&self, id: Id, path: &str) -> AppResult<Vec<SftpEntryView>> {
        let mut entries = self.get(id)?.list(path).await?;
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries.into_iter().map(Into::into).collect())
    }

    pub async fn mkdir(&self, id: Id, path: &str) -> AppResult<()> {
        Ok(self.get(id)?.mkdir(path).await?)
    }

    pub async fn rename(&self, id: Id, from: &str, to: &str) -> AppResult<()> {
        Ok(self.get(id)?.rename(from, to).await?)
    }

    pub async fn remove(&self, id: Id, path: &str, is_dir: bool) -> AppResult<()> {
        Ok(self.get(id)?.remove(path, is_dir).await?)
    }

    pub async fn download(&self, id: Id, remote: &str, local: &Path) -> AppResult<()> {
        Ok(self.get(id)?.download(remote, local).await?)
    }

    pub async fn upload(&self, id: Id, local: &Path, remote_dir: &str) -> AppResult<String> {
        let name = local
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| DomainError::Invalid("local path has no file name".into()))?;
        let remote = join_remote(remote_dir, name);
        self.get(id)?.upload(local, &remote).await?;
        Ok(remote)
    }

    pub async fn close(&self, id: Id) {
        if let Some((_, fs)) = self.sessions.remove(&id) {
            fs.close().await;
        }
    }

    fn get(&self, id: Id) -> AppResult<Arc<dyn RemoteFs>> {
        self.sessions
            .get(&id)
            .map(|fs| Arc::clone(fs.value()))
            .ok_or(AppError::NotFound("sftp session"))
    }
}

pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::join_remote;

    #[test]
    fn joins_paths() {
        assert_eq!(join_remote("/home/a", "f.txt"), "/home/a/f.txt");
        assert_eq!(join_remote("/", "f.txt"), "/f.txt");
    }
}
