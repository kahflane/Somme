use std::sync::Arc;

use somme_domain::DomainError;

use crate::dto::{AccountStatus, SyncStatusView};
use crate::error::AppResult;
use crate::ports::{CloudAccounts, CloudSession, EventPublisher, SessionStore, SyncGateway};
use crate::services::VaultService;

const MIN_PASSWORD_LEN: usize = 10;

pub struct SyncService {
    gateway: Arc<dyn SyncGateway>,
    cloud: Arc<dyn CloudAccounts>,
    sessions: Arc<dyn SessionStore>,
    vault: Arc<VaultService>,
    events: Arc<dyn EventPublisher>,
}

impl SyncService {
    pub fn new(
        gateway: Arc<dyn SyncGateway>,
        cloud: Arc<dyn CloudAccounts>,
        sessions: Arc<dyn SessionStore>,
        vault: Arc<VaultService>,
        events: Arc<dyn EventPublisher>,
    ) -> Self {
        Self {
            gateway,
            cloud,
            sessions,
            vault,
            events,
        }
    }

    pub fn status(&self) -> SyncStatusView {
        self.gateway.state().into()
    }

    pub fn account(&self) -> AccountStatus {
        let session = self.sessions.load();
        AccountStatus {
            signed_in: session.is_some(),
            email: session.map(|s| s.email),
        }
    }

    pub async fn register(&self, email: &str, password: &str) -> AppResult<AccountStatus> {
        let (email, password) = validate(email, password)?;
        let session = self.cloud.register(&email, password).await?;
        self.start(session).await
    }

    pub async fn login(&self, email: &str, password: &str) -> AppResult<AccountStatus> {
        let (email, password) = validate(email, password)?;
        let session = self.cloud.login(&email, password).await?;
        self.start(session).await
    }

    pub async fn logout(&self) -> AppResult<AccountStatus> {
        self.gateway.disconnect().await?;
        if let Some(session) = self.sessions.load()
            && let Err(err) = self.cloud.logout(&session).await {
                tracing::warn!(%err, "remote sign out failed");
            }
        self.sessions.clear()?;
        Ok(self.account())
    }

    pub async fn sync_now(&self) -> AppResult<SyncStatusView> {
        self.gateway.sync_now().await?;
        Ok(self.status())
    }

    async fn start(&self, session: CloudSession) -> AppResult<AccountStatus> {
        self.sessions.save(&session)?;
        let credentials = match self.cloud.credentials(&session).await {
            Ok(credentials) => credentials,
            Err(err) => {
                self.sessions.clear()?;
                return Err(err.into());
            }
        };
        self.gateway.connect(&credentials.url).await?;
        self.vault.revalidate().await?;
        self.events.data_changed();
        Ok(self.account())
    }
}

fn validate<'a>(email: &str, password: &'a str) -> AppResult<(String, &'a str)> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(DomainError::Invalid("enter a valid email address".into()).into());
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(DomainError::Invalid(format!(
            "password must have at least {MIN_PASSWORD_LEN} characters"
        ))
        .into());
    }
    Ok((email, password))
}
