use std::sync::Arc;

use async_trait::async_trait;
use somme_domain::{
    Automation, Group, Host, Identity, KnownHostRepository, PortForward, Repository, Snippet, SshKey, VaultRepository,
};

use crate::ports::GatewayError;

pub trait VaultData:
    Repository<Group>
    + Repository<SshKey>
    + Repository<Identity>
    + Repository<Snippet>
    + Repository<Automation>
    + Repository<Host>
    + Repository<PortForward>
    + KnownHostRepository
    + VaultRepository
{
}

impl<T> VaultData for T where
    T: Repository<Group>
        + Repository<SshKey>
        + Repository<Identity>
        + Repository<Snippet>
        + Repository<Automation>
        + Repository<Host>
        + Repository<PortForward>
        + KnownHostRepository
        + VaultRepository
        + ?Sized
{
}

#[derive(Debug, Clone, Default)]
pub struct SyncState {
    pub enabled: bool,
    pub url: Option<String>,
    pub syncing: bool,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    Merged,
    RemoteKept,
}

#[async_trait]
pub trait SyncGateway: Send + Sync {
    fn state(&self) -> SyncState;
    async fn connect(&self, url: &str) -> Result<SyncOutcome, GatewayError>;
    async fn disconnect(&self) -> Result<(), GatewayError>;
    async fn sync_now(&self) -> Result<(), GatewayError>;
    async fn previous_data(&self) -> Result<Option<Arc<dyn VaultData>>, GatewayError>;
    async fn forget_previous_data(&self) -> Result<(), GatewayError>;
}
