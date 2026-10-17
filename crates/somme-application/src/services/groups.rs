use std::sync::Arc;

use somme_domain::{Group, Id, Repository, new_id, now};

use crate::dto::{GroupInput, GroupView};
use crate::error::{AppError, AppResult};
use crate::services::Links;
use crate::services::secrets::clean;

pub struct GroupService {
    repo: Arc<dyn Repository<Group>>,
    links: Arc<Links>,
}

impl GroupService {
    pub fn new(repo: Arc<dyn Repository<Group>>, links: Arc<Links>) -> Self {
        Self { repo, links }
    }

    pub async fn list(&self) -> AppResult<Vec<GroupView>> {
        let mut groups = self.repo.list().await?;
        groups.sort_by_key(|g| g.name.to_lowercase());
        Ok(groups.into_iter().map(Into::into).collect())
    }

    pub async fn save(&self, input: GroupInput) -> AppResult<GroupView> {
        let stamp = now();
        let existing = match input.id {
            Some(id) => Some(self.repo.get(id).await?.ok_or(AppError::NotFound("group"))?),
            None => None,
        };
        let group = Group {
            id: existing.as_ref().map_or_else(new_id, |g| g.id),
            name: clean(&input.name),
            parent_id: input.parent_id,
            created_at: existing.as_ref().map_or(stamp, |g| g.created_at),
            updated_at: stamp,
        };
        group.validate()?;
        self.repo.save(&group).await?;
        Ok(group.into())
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.repo.delete(id).await?;
        self.links.detach(id).await
    }
}
