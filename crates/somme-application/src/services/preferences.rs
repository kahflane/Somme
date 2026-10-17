use std::sync::Arc;

use somme_domain::PreferenceRepository;

use crate::error::AppResult;

pub struct PreferenceService {
    repo: Arc<dyn PreferenceRepository>,
}

impl PreferenceService {
    pub fn new(repo: Arc<dyn PreferenceRepository>) -> Self {
        Self { repo }
    }

    pub async fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.repo.preference(key).await?)
    }

    pub async fn set(&self, key: &str, value: &str) -> AppResult<()> {
        Ok(self.repo.set_preference(key, value).await?)
    }
}
