use std::sync::Arc;

use somme_domain::{DEFAULT_SSH_PORT, Host, new_id, now};

use crate::dto::KeyImportInput;
use crate::error::AppResult;
use crate::ports::SshConfigSource;
use crate::services::{HostService, KeyService};

pub struct ImportService {
    source: Arc<dyn SshConfigSource>,
    hosts: Arc<HostService>,
    keys: Arc<KeyService>,
}

impl ImportService {
    pub fn new(source: Arc<dyn SshConfigSource>, hosts: Arc<HostService>, keys: Arc<KeyService>) -> Self {
        Self { source, hosts, keys }
    }

    pub async fn import_ssh_config(&self) -> AppResult<usize> {
        let existing = self.hosts.all().await?;
        let mut imported = 0;
        for entry in self.source.load()? {
            let port = entry.port.unwrap_or(DEFAULT_SSH_PORT);
            let duplicate = existing.iter().any(|h| {
                h.address == entry.address && h.port == port && h.username == entry.username
            });
            if duplicate {
                continue;
            }
            let mut key_id = None;
            for (label, private_key) in entry.private_keys {
                let input = KeyImportInput {
                    label,
                    private_key,
                    passphrase: None,
                };
                match self.keys.import(input).await {
                    Ok(key) => {
                        key_id = Some(key.id);
                        break;
                    }
                    Err(err) => tracing::warn!(%err, "skipped key from ssh config"),
                }
            }
            let stamp = now();
            let host = Host {
                id: new_id(),
                label: entry.alias,
                address: entry.address,
                port,
                group_id: None,
                identity_id: None,
                username: entry.username,
                password: None,
                key_id,
                jump_host_id: None,
                startup_snippet_id: None,
                tags: vec!["ssh-config".into()],
                color: None,
                created_at: stamp,
                updated_at: stamp,
            };
            match self.hosts.create(host).await {
                Ok(_) => imported += 1,
                Err(err) => tracing::warn!(%err, "skipped host from ssh config"),
            }
        }
        Ok(imported)
    }
}
