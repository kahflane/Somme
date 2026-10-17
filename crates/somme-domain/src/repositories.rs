use async_trait::async_trait;

use crate::entities::{KnownHost, VaultMeta};
use crate::error::RepositoryError;
use crate::values::Id;

#[async_trait]
pub trait Repository<T>: Send + Sync {
    async fn list(&self) -> Result<Vec<T>, RepositoryError>;
    async fn get(&self, id: Id) -> Result<Option<T>, RepositoryError>;
    async fn save(&self, item: &T) -> Result<(), RepositoryError>;
    async fn delete(&self, id: Id) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait KnownHostRepository: Repository<KnownHost> {
    async fn find(&self, host: &str, port: u16) -> Result<Option<KnownHost>, RepositoryError>;
}

#[async_trait]
pub trait PreferenceRepository: Send + Sync {
    async fn preference(&self, key: &str) -> Result<Option<String>, RepositoryError>;
    async fn set_preference(&self, key: &str, value: &str) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait VaultRepository: Send + Sync {
    async fn load(&self) -> Result<Option<VaultMeta>, RepositoryError>;
    async fn store(&self, meta: &VaultMeta) -> Result<(), RepositoryError>;
}
