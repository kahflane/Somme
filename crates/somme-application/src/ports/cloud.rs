use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::ports::GatewayError;

#[derive(Clone)]
pub struct CloudSession {
    pub token: Zeroizing<String>,
    pub email: String,
}

#[derive(Clone)]
pub struct SyncCredentials {
    pub url: String,
    pub token: Zeroizing<String>,
    pub expires_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Rejected(String),
    #[error("session expired, sign in again")]
    SessionExpired,
    #[error("cannot reach Somme Cloud: {0}")]
    Unreachable(String),
}

impl From<CloudError> for GatewayError {
    fn from(err: CloudError) -> Self {
        Self::Connection(err.to_string())
    }
}

#[async_trait]
pub trait CloudAccounts: Send + Sync {
    async fn register(&self, email: &str, password: &str) -> Result<CloudSession, CloudError>;
    async fn login(&self, email: &str, password: &str) -> Result<CloudSession, CloudError>;
    async fn logout(&self, session: &CloudSession) -> Result<(), CloudError>;
    async fn credentials(&self, session: &CloudSession) -> Result<SyncCredentials, CloudError>;
}

pub trait SessionStore: Send + Sync {
    fn load(&self) -> Option<CloudSession>;
    fn save(&self, session: &CloudSession) -> Result<(), GatewayError>;
    fn clear(&self) -> Result<(), GatewayError>;
}
