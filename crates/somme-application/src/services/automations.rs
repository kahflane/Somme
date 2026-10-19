use std::sync::Arc;

use somme_domain::{Automation, Id, Repository, new_id, now};

use crate::dto::{AutomationInput, AutomationView};
use crate::error::{AppError, AppResult};
use crate::services::VaultService;

/// Automations, sealed with the vault when saved and opened when listed.
pub struct AutomationService {
    repo: Arc<dyn Repository<Automation>>,
    vault: Arc<VaultService>,
}

impl AutomationService {
    pub fn new(repo: Arc<dyn Repository<Automation>>, vault: Arc<VaultService>) -> Self {
        Self { repo, vault }
    }

    /// Every automation the vault can open, oldest first.
    pub async fn list(&self) -> AppResult<Vec<AutomationView>> {
        if !self.vault.is_unlocked() {
            return Err(AppError::VaultLocked);
        }
        let mut items = self.repo.list().await?;
        items.sort_by_key(|a| a.created_at);
        Ok(items.into_iter().filter_map(|a| self.view(a).ok()).collect())
    }

    pub async fn get(&self, id: Id) -> AppResult<Option<AutomationView>> {
        match self.repo.get(id).await? {
            Some(automation) => self.view(automation).map(Some),
            None => Ok(None),
        }
    }

    pub async fn save(&self, input: AutomationInput) -> AppResult<AutomationView> {
        if input.document.trim().is_empty() {
            return Err(AppError::Domain(somme_domain::DomainError::Required("document")));
        }
        let stamp = now();
        let existing = match input.id {
            Some(id) => self.repo.get(id).await?,
            None => None,
        };
        let automation = Automation {
            id: existing.as_ref().map(|a| a.id).or(input.id).unwrap_or_else(new_id),
            document: self.vault.seal(&input.document)?,
            created_at: existing.as_ref().map_or(stamp, |a| a.created_at),
            updated_at: stamp,
        };
        self.repo.save(&automation).await?;
        self.view(automation)
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        Ok(self.repo.delete(id).await?)
    }

    fn view(&self, automation: Automation) -> AppResult<AutomationView> {
        let document = self.vault.open(&automation.document)?;
        Ok(AutomationView {
            id: automation.id,
            document: document.to_string(),
            created_at: automation.created_at,
            updated_at: automation.updated_at,
        })
    }
}
