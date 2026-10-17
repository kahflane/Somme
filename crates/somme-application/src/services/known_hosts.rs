use std::sync::Arc;

use async_trait::async_trait;
use somme_domain::{Id, KnownHost, KnownHostRepository, new_id, now};

use crate::dto::KnownHostView;
use crate::error::AppResult;
use crate::ports::{HostKeyVerifier, PromptAnswer, PromptKind, Prompter, ServerKey};

pub struct KnownHostService {
    repo: Arc<dyn KnownHostRepository>,
}

impl KnownHostService {
    pub fn new(repo: Arc<dyn KnownHostRepository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> AppResult<Vec<KnownHostView>> {
        let mut items = self.repo.list().await?;
        items.sort_by(|a, b| a.host.cmp(&b.host).then(a.port.cmp(&b.port)));
        Ok(items.into_iter().map(Into::into).collect())
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        Ok(self.repo.delete(id).await?)
    }
}

pub struct KnownHostsVerifier {
    repo: Arc<dyn KnownHostRepository>,
    prompter: Arc<dyn Prompter>,
}

impl KnownHostsVerifier {
    pub fn new(repo: Arc<dyn KnownHostRepository>, prompter: Arc<dyn Prompter>) -> Self {
        Self { repo, prompter }
    }

    async fn check(&self, title: &str, host: &str, port: u16, key: &ServerKey) -> AppResult<bool> {
        let existing = self.repo.find(host, port).await?;
        if existing.as_ref().is_some_and(|k| k.matches(&key.fingerprint)) {
            return Ok(true);
        }
        let answer = self
            .prompter
            .ask(
                title,
                PromptKind::HostKey {
                    host: host.to_owned(),
                    port,
                    key_type: key.key_type.clone(),
                    fingerprint: key.fingerprint.clone(),
                    previous_fingerprint: existing.as_ref().map(|k| k.fingerprint.clone()),
                },
            )
            .await;
        if answer != PromptAnswer::Accept {
            return Ok(false);
        }
        let record = KnownHost {
            id: existing.as_ref().map_or_else(new_id, |k| k.id),
            host: host.to_owned(),
            port,
            key_type: key.key_type.clone(),
            fingerprint: key.fingerprint.clone(),
            public_key: key.public_key.clone(),
            created_at: now(),
        };
        self.repo.save(&record).await?;
        Ok(true)
    }
}

#[async_trait]
impl HostKeyVerifier for KnownHostsVerifier {
    async fn verify(&self, title: &str, host: &str, port: u16, key: &ServerKey) -> bool {
        match self.check(title, host, port, key).await {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::error!(%err, host, port, "host key check failed");
                false
            }
        }
    }
}
