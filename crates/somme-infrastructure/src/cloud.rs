use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use somme_application::ports::{
    CloudAccounts, CloudError, CloudSession, GatewayError, SessionStore, SyncCredentials,
};
use somme_domain::now;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::db::TokenSource;
use crate::keystore::KeyringStore;

const SESSION_USER: &str = "cloud-session";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;
pub const DEFAULT_CLOUD_URL: &str = "http://127.0.0.1:8787";

#[derive(Serialize)]
struct AuthBody<'a> {
    email: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct SessionBody {
    token: String,
    email: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsBody {
    url: String,
    token: String,
    expires_at: i64,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

pub struct HttpCloudClient {
    http: reqwest::Client,
    base: String,
}

fn unreachable(err: impl std::fmt::Display) -> CloudError {
    CloudError::Unreachable(err.to_string())
}

impl HttpCloudClient {
    pub fn new(base: &str) -> Result<Self, CloudError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("somme/", env!("CARGO_PKG_VERSION")))
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(unreachable)?,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    async fn rejection(response: reqwest::Response) -> CloudError {
        let status = response.status();
        let message = response
            .json::<ErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_else(|_| format!("Somme Cloud returned {status}"));
        if status.is_server_error() {
            CloudError::Unreachable(message)
        } else {
            CloudError::Rejected(message)
        }
    }

    async fn auth(&self, path: &str, email: &str, password: &str) -> Result<CloudSession, CloudError> {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&AuthBody { email, password })
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(Self::rejection(response).await);
        }
        let body: SessionBody = response.json().await.map_err(unreachable)?;
        Ok(CloudSession {
            token: Zeroizing::new(body.token),
            email: body.email,
        })
    }
}

#[async_trait]
impl CloudAccounts for HttpCloudClient {
    async fn register(&self, email: &str, password: &str) -> Result<CloudSession, CloudError> {
        self.auth("/v1/accounts", email, password).await
    }

    async fn login(&self, email: &str, password: &str) -> Result<CloudSession, CloudError> {
        self.auth("/v1/sessions", email, password).await
    }

    async fn logout(&self, session: &CloudSession) -> Result<(), CloudError> {
        let response = self
            .http
            .delete(format!("{}/v1/sessions/current", self.base))
            .bearer_auth(session.token.as_str())
            .send()
            .await
            .map_err(unreachable)?;
        if response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED {
            Ok(())
        } else {
            Err(Self::rejection(response).await)
        }
    }

    async fn credentials(&self, session: &CloudSession) -> Result<SyncCredentials, CloudError> {
        let response = self
            .http
            .post(format!("{}/v1/sync/credentials", self.base))
            .bearer_auth(session.token.as_str())
            .send()
            .await
            .map_err(unreachable)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(CloudError::SessionExpired);
        }
        if !response.status().is_success() {
            return Err(Self::rejection(response).await);
        }
        let body: CredentialsBody = response.json().await.map_err(unreachable)?;
        Ok(SyncCredentials {
            url: body.url,
            token: Zeroizing::new(body.token),
            expires_at: body.expires_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    token: String,
    email: String,
}

pub struct KeyringSessionStore {
    keyring: KeyringStore,
}

impl Default for KeyringSessionStore {
    fn default() -> Self {
        Self {
            keyring: KeyringStore::new(SESSION_USER),
        }
    }
}

impl SessionStore for KeyringSessionStore {
    fn load(&self) -> Option<CloudSession> {
        let raw = self.keyring.get_secret()?;
        let stored: StoredSession = serde_json::from_str(&raw).ok()?;
        Some(CloudSession {
            token: Zeroizing::new(stored.token),
            email: stored.email,
        })
    }

    fn save(&self, session: &CloudSession) -> Result<(), GatewayError> {
        let raw = Zeroizing::new(
            serde_json::to_string(&StoredSession {
                token: session.token.to_string(),
                email: session.email.clone(),
            })
            .map_err(|e| GatewayError::Io(e.to_string()))?,
        );
        self.keyring
            .set_secret(&raw)
            .map_err(|e| GatewayError::Io(e.to_string()))
    }

    fn clear(&self) -> Result<(), GatewayError> {
        self.keyring
            .delete_secret()
            .map_err(|e| GatewayError::Io(e.to_string()))
    }
}

pub struct ManagedTokens {
    cloud: Arc<dyn CloudAccounts>,
    sessions: Arc<dyn SessionStore>,
    cached: Mutex<Option<SyncCredentials>>,
}

impl ManagedTokens {
    pub fn new(cloud: Arc<dyn CloudAccounts>, sessions: Arc<dyn SessionStore>) -> Self {
        Self {
            cloud,
            sessions,
            cached: Mutex::new(None),
        }
    }
}

#[async_trait]
impl TokenSource for ManagedTokens {
    async fn token(&self) -> Option<String> {
        let session = self.sessions.load()?;
        let mut cached = self.cached.lock().await;
        if let Some(credentials) = cached.as_ref().filter(|c| c.expires_at - REFRESH_MARGIN_MS > now()) {
            return Some(credentials.token.to_string());
        }
        match self.cloud.credentials(&session).await {
            Ok(credentials) => {
                let token = credentials.token.to_string();
                *cached = Some(credentials);
                Some(token)
            }
            Err(err) => {
                tracing::warn!(%err, "could not refresh sync token");
                *cached = None;
                None
            }
        }
    }

    fn available(&self) -> bool {
        self.sessions.load().is_some()
    }
}
