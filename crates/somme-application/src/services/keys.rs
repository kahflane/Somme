use std::sync::Arc;

use somme_domain::{Id, KeyAlgorithm, Repository, SshKey, new_id, now};

use crate::dto::{KeyGenerateInput, KeyImportInput, KeyView};
use crate::error::{AppError, AppResult};
use crate::ports::KeyTool;
use crate::services::secrets::{clean, clean_opt};
use crate::services::{Links, VaultService};

pub struct KeyService {
    repo: Arc<dyn Repository<SshKey>>,
    tool: Arc<dyn KeyTool>,
    vault: Arc<VaultService>,
    links: Arc<Links>,
}

impl KeyService {
    pub fn new(
        repo: Arc<dyn Repository<SshKey>>,
        tool: Arc<dyn KeyTool>,
        vault: Arc<VaultService>,
        links: Arc<Links>,
    ) -> Self {
        Self {
            repo,
            tool,
            vault,
            links,
        }
    }

    pub async fn list(&self) -> AppResult<Vec<KeyView>> {
        let mut keys = self.repo.list().await?;
        keys.sort_by_key(|k| k.label.to_lowercase());
        Ok(keys.into_iter().map(Into::into).collect())
    }

    pub async fn import(&self, input: KeyImportInput) -> AppResult<KeyView> {
        let passphrase = clean_opt(input.passphrase);
        let private_key = format!("{}\n", input.private_key.trim());
        let info = self.tool.inspect(&private_key, passphrase.as_deref())?;
        if let Some(existing) = self
            .repo
            .list()
            .await?
            .into_iter()
            .find(|k| k.fingerprint == info.fingerprint)
        {
            return Ok(existing.into());
        }
        let stamp = now();
        let key = SshKey {
            id: new_id(),
            label: clean(&input.label),
            algorithm: info.algorithm,
            public_key: info.public_key,
            fingerprint: info.fingerprint,
            private_key: self.vault.seal(&private_key)?,
            passphrase: passphrase.map(|p| self.vault.seal(&p)).transpose()?,
            created_at: stamp,
            updated_at: stamp,
        };
        key.validate()?;
        self.repo.save(&key).await?;
        Ok(key.into())
    }

    pub async fn generate(&self, input: KeyGenerateInput) -> AppResult<KeyView> {
        let algorithm: KeyAlgorithm = input.algorithm.parse()?;
        let label = clean(&input.label);
        let generated = self.tool.generate(algorithm, &label)?;
        let stamp = now();
        let key = SshKey {
            id: new_id(),
            label,
            algorithm: generated.info.algorithm,
            public_key: generated.info.public_key,
            fingerprint: generated.info.fingerprint,
            private_key: self.vault.seal(&generated.private_key)?,
            passphrase: None,
            created_at: stamp,
            updated_at: stamp,
        };
        key.validate()?;
        self.repo.save(&key).await?;
        Ok(key.into())
    }

    pub async fn rename(&self, id: Id, label: &str) -> AppResult<KeyView> {
        let mut key = self.repo.get(id).await?.ok_or(AppError::NotFound("key"))?;
        key.label = clean(label);
        key.updated_at = now();
        key.validate()?;
        self.repo.save(&key).await?;
        Ok(key.into())
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.repo.delete(id).await?;
        self.links.detach(id).await
    }
}
