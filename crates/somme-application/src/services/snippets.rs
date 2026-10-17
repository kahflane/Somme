use std::sync::Arc;

use somme_domain::{Id, Repository, Snippet, new_id, now};

use crate::dto::{SnippetInput, SnippetView};
use crate::error::{AppError, AppResult};
use crate::services::Links;
use crate::services::secrets::clean;

pub struct SnippetService {
    repo: Arc<dyn Repository<Snippet>>,
    links: Arc<Links>,
}

impl SnippetService {
    pub fn new(repo: Arc<dyn Repository<Snippet>>, links: Arc<Links>) -> Self {
        Self { repo, links }
    }

    pub async fn list(&self) -> AppResult<Vec<SnippetView>> {
        let mut items = self.repo.list().await?;
        items.sort_by_key(|s| s.label.to_lowercase());
        Ok(items.into_iter().map(Into::into).collect())
    }

    pub async fn save(&self, input: SnippetInput) -> AppResult<SnippetView> {
        let stamp = now();
        let existing = match input.id {
            Some(id) => Some(self.repo.get(id).await?.ok_or(AppError::NotFound("snippet"))?),
            None => None,
        };
        let snippet = Snippet {
            id: existing.as_ref().map_or_else(new_id, |s| s.id),
            label: clean(&input.label),
            script: input.script.trim_end().to_owned(),
            created_at: existing.as_ref().map_or(stamp, |s| s.created_at),
            updated_at: stamp,
        };
        snippet.validate()?;
        self.repo.save(&snippet).await?;
        Ok(snippet.into())
    }

    pub async fn get(&self, id: Id) -> AppResult<Option<Snippet>> {
        Ok(self.repo.get(id).await?)
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.repo.delete(id).await?;
        self.links.detach(id).await
    }
}
