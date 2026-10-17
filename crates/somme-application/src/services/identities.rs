use std::sync::Arc;

use somme_domain::{Id, Identity, Repository, new_id, now};

use crate::dto::{IdentityInput, IdentityView};
use crate::error::{AppError, AppResult};
use crate::services::secrets::{apply_secret, clean};
use crate::services::{Links, VaultService};

pub struct IdentityService {
    repo: Arc<dyn Repository<Identity>>,
    vault: Arc<VaultService>,
    links: Arc<Links>,
}

impl IdentityService {
    pub fn new(
        repo: Arc<dyn Repository<Identity>>,
        vault: Arc<VaultService>,
        links: Arc<Links>,
    ) -> Self {
        Self { repo, vault, links }
    }

    pub async fn list(&self) -> AppResult<Vec<IdentityView>> {
        let mut items = self.repo.list().await?;
        items.sort_by_key(|i| i.label.to_lowercase());
        Ok(items.into_iter().map(Into::into).collect())
    }

    pub async fn save(&self, input: IdentityInput) -> AppResult<IdentityView> {
        let stamp = now();
        let existing = match input.id {
            Some(id) => Some(self.repo.get(id).await?.ok_or(AppError::NotFound("identity"))?),
            None => None,
        };
        let password = apply_secret(
            &self.vault,
            existing.as_ref().and_then(|i| i.password.clone()),
            input.password,
        )?;
        let identity = Identity {
            id: existing.as_ref().map_or_else(new_id, |i| i.id),
            label: clean(&input.label),
            username: clean(&input.username),
            password,
            key_id: input.key_id,
            created_at: existing.as_ref().map_or(stamp, |i| i.created_at),
            updated_at: stamp,
        };
        identity.validate()?;
        self.repo.save(&identity).await?;
        Ok(identity.into())
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.repo.delete(id).await?;
        self.links.detach(id).await
    }
}
