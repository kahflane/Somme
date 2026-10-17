use async_trait::async_trait;

use crate::ports::GatewayError;

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
}
