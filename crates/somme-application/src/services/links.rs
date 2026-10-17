use std::sync::Arc;

use somme_domain::{Group, Host, Id, Identity, Repository, now};

use crate::error::AppResult;

pub struct Links {
    hosts: Arc<dyn Repository<Host>>,
    identities: Arc<dyn Repository<Identity>>,
    groups: Arc<dyn Repository<Group>>,
}

impl Links {
    pub fn new(
        hosts: Arc<dyn Repository<Host>>,
        identities: Arc<dyn Repository<Identity>>,
        groups: Arc<dyn Repository<Group>>,
    ) -> Self {
        Self {
            hosts,
            identities,
            groups,
        }
    }

    pub async fn detach(&self, id: Id) -> AppResult<()> {
        let stamp = now();
        for mut host in self.hosts.list().await? {
            if host.detach(id) {
                host.updated_at = stamp;
                self.hosts.save(&host).await?;
            }
        }
        for mut identity in self.identities.list().await? {
            if identity.key_id == Some(id) {
                identity.key_id = None;
                identity.updated_at = stamp;
                self.identities.save(&identity).await?;
            }
        }
        for mut group in self.groups.list().await? {
            if group.parent_id == Some(id) {
                group.parent_id = None;
                group.updated_at = stamp;
                self.groups.save(&group).await?;
            }
        }
        Ok(())
    }
}
