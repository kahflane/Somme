use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::ports::GatewayError;

/// The GitHub user behind a Somme Cloud account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloudProfile {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Clone)]
pub struct CloudSession {
    pub token: Zeroizing<String>,
    pub profile: CloudProfile,
}

#[derive(Clone)]
pub struct SyncCredentials {
    pub url: String,
    pub token: Zeroizing<String>,
    pub expires_at: i64,
}

/// A GitHub token for the signed-in user's repositories, kept on this
/// computer only.
pub struct GitHubAccess {
    pub token: Zeroizing<String>,
    /// The scopes the user granted, comma separated.
    pub scope: String,
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
    /// Opens GitHub in the browser and waits for the user to sign in there.
    async fn sign_in_with_github(&self) -> Result<CloudSession, CloudError>;
    async fn logout(&self, session: &CloudSession) -> Result<(), CloudError>;
    async fn credentials(&self, session: &CloudSession) -> Result<SyncCredentials, CloudError>;
    /// Downloads a profile picture.
    async fn avatar(&self, url: &str) -> Result<Vec<u8>, CloudError>;
    /// Opens GitHub in the browser to let Somme read and act on the user's
    /// repositories, and waits for the token.
    async fn github_access(&self, session: &CloudSession) -> Result<GitHubAccess, CloudError>;
}

/// Keeps one secret, like the GitHub token, in the system keychain.
pub trait SecretStore: Send + Sync {
    fn load(&self) -> Option<Zeroizing<String>>;
    fn save(&self, secret: &str) -> Result<(), GatewayError>;
    fn clear(&self) -> Result<(), GatewayError>;
}

pub trait SessionStore: Send + Sync {
    fn load(&self) -> Option<CloudSession>;
    /// Who is signed in. Unlike `load`, this may answer without the secret store.
    fn profile(&self) -> Option<CloudProfile> {
        self.load().map(|session| session.profile)
    }
    fn save(&self, session: &CloudSession) -> Result<(), GatewayError>;
    fn clear(&self) -> Result<(), GatewayError>;
}
