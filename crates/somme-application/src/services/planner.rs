use std::collections::HashSet;
use std::sync::Arc;

use somme_domain::{DomainError, Host, Id, Identity, Repository, SshKey};

use crate::error::{AppError, AppResult};
use crate::ports::{ConnectPlan, Endpoint, KeyCredential};
use crate::services::VaultService;
use crate::services::hosts::MAX_JUMP_HOPS;

pub struct ConnectionPlanner {
    hosts: Arc<dyn Repository<Host>>,
    identities: Arc<dyn Repository<Identity>>,
    keys: Arc<dyn Repository<SshKey>>,
    vault: Arc<VaultService>,
}

impl ConnectionPlanner {
    pub fn new(
        hosts: Arc<dyn Repository<Host>>,
        identities: Arc<dyn Repository<Identity>>,
        keys: Arc<dyn Repository<SshKey>>,
        vault: Arc<VaultService>,
    ) -> Self {
        Self {
            hosts,
            identities,
            keys,
            vault,
        }
    }

    pub async fn plan(&self, host_id: Id) -> AppResult<(Host, ConnectPlan)> {
        let chain = self.chain(host_id).await?;
        let mut hops = Vec::with_capacity(chain.len());
        for host in &chain {
            hops.push(self.endpoint(host).await?);
        }
        let target = chain.last().cloned().ok_or(AppError::NotFound("host"))?;
        Ok((target, ConnectPlan { hops }))
    }

    async fn chain(&self, host_id: Id) -> AppResult<Vec<Host>> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(host_id);
        while let Some(id) = next {
            if !seen.insert(id) || chain.len() > MAX_JUMP_HOPS {
                return Err(AppError::Conflict("jump hosts form a loop or are too deep".into()));
            }
            let host = self.hosts.get(id).await?.ok_or(AppError::NotFound("host"))?;
            next = host.jump_host_id;
            chain.push(host);
        }
        chain.reverse();
        Ok(chain)
    }

    async fn endpoint(&self, host: &Host) -> AppResult<Endpoint> {
        let identity = match host.identity_id {
            Some(id) => self.identities.get(id).await?,
            None => None,
        };
        let username = identity
            .as_ref()
            .map(|i| i.username.clone())
            .or_else(|| host.username.clone())
            .ok_or(DomainError::Required("username"))?;
        let sealed_password = identity
            .as_ref()
            .and_then(|i| i.password.clone())
            .or_else(|| host.password.clone());
        let password = sealed_password.map(|p| self.vault.open(&p)).transpose()?;
        let mut key_ids: Vec<Id> = identity.as_ref().and_then(|i| i.key_id).into_iter().collect();
        if let Some(id) = host.key_id.filter(|id| !key_ids.contains(id)) {
            key_ids.push(id);
        }
        let mut keys = Vec::with_capacity(key_ids.len());
        for id in key_ids {
            if let Some(key) = self.keys.get(id).await? {
                keys.push(KeyCredential {
                    label: key.label.clone(),
                    private_key: self.vault.open(&key.private_key)?,
                    passphrase: key.passphrase.as_ref().map(|p| self.vault.open(p)).transpose()?,
                });
            }
        }
        Ok(Endpoint {
            title: host.display_name().to_owned(),
            host: host.address.clone(),
            port: host.port,
            username,
            password,
            keys,
        })
    }
}
