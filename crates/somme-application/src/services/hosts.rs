use std::collections::HashSet;
use std::sync::Arc;

use somme_domain::{Host, Id, Repository, new_id, now};

use crate::dto::{HostInput, HostView};
use crate::error::{AppError, AppResult};
use crate::services::secrets::{apply_secret, clean, clean_opt};
use crate::services::{ForwardService, Links, VaultService};

pub const MAX_JUMP_HOPS: usize = 8;

pub struct HostService {
    repo: Arc<dyn Repository<Host>>,
    vault: Arc<VaultService>,
    links: Arc<Links>,
    forwards: Arc<ForwardService>,
}

impl HostService {
    pub fn new(
        repo: Arc<dyn Repository<Host>>,
        vault: Arc<VaultService>,
        links: Arc<Links>,
        forwards: Arc<ForwardService>,
    ) -> Self {
        Self {
            repo,
            vault,
            links,
            forwards,
        }
    }

    pub async fn list(&self) -> AppResult<Vec<HostView>> {
        let mut hosts = self.repo.list().await?;
        hosts.sort_by_key(|h| h.display_name().to_lowercase());
        Ok(hosts.into_iter().map(Into::into).collect())
    }

    pub async fn save(&self, input: HostInput) -> AppResult<HostView> {
        let stamp = now();
        let existing = match input.id {
            Some(id) => Some(self.repo.get(id).await?.ok_or(AppError::NotFound("host"))?),
            None => None,
        };
        let password = apply_secret(
            &self.vault,
            existing.as_ref().and_then(|h| h.password.clone()),
            input.password,
        )?;
        let host = Host {
            id: existing.as_ref().map_or_else(new_id, |h| h.id),
            label: clean(&input.label),
            address: clean(&input.address),
            port: input.port,
            group_id: input.group_id,
            identity_id: input.identity_id,
            username: clean_opt(input.username),
            password,
            key_id: input.key_id,
            jump_host_id: input.jump_host_id,
            startup_snippet_id: input.startup_snippet_id,
            tags: input
                .tags
                .into_iter()
                .filter_map(|t| clean_opt(Some(t)))
                .collect(),
            color: clean_opt(input.color),
            created_at: existing.as_ref().map_or(stamp, |h| h.created_at),
            updated_at: stamp,
        };
        host.validate()?;
        self.ensure_no_jump_loop(&host).await?;
        self.repo.save(&host).await?;
        Ok(host.into())
    }

    pub async fn create(&self, host: Host) -> AppResult<Host> {
        host.validate()?;
        self.repo.save(&host).await?;
        Ok(host)
    }

    pub async fn all(&self) -> AppResult<Vec<Host>> {
        Ok(self.repo.list().await?)
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.forwards.delete_for_host(id).await?;
        self.repo.delete(id).await?;
        self.links.detach(id).await
    }

    async fn ensure_no_jump_loop(&self, host: &Host) -> AppResult<()> {
        let mut seen = HashSet::from([host.id]);
        let mut next = host.jump_host_id;
        while let Some(id) = next {
            if !seen.insert(id) || seen.len() > MAX_JUMP_HOPS + 1 {
                return Err(AppError::Conflict("jump hosts form a loop or are too deep".into()));
            }
            next = self
                .repo
                .get(id)
                .await?
                .ok_or(AppError::NotFound("jump host"))?
                .jump_host_id;
        }
        Ok(())
    }
}
