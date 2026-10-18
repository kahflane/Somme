use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use somme_application::ports::{
    CloudAccounts, CloudError, CloudProfile, CloudSession, GatewayError, GitHubAccess, SessionStore, SyncCredentials,
};
use somme_domain::now;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::db::TokenSource;
use crate::github_login;
use crate::keystore::KeyringStore;

const SESSION_USER: &str = "cloud-session";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;
/// The hosted Somme Cloud. Set `SOMME_CLOUD_URL` to use a local one.
pub const DEFAULT_CLOUD_URL: &str = "https://somme-cloud.fly.dev";
/// Profile pictures only come from GitHub.
const AVATAR_HOST: &str = "avatars.githubusercontent.com";
const AVATAR_SIZE: &str = "96";
/// Read and write access to the user's repositories, for pull requests and issues.
const REPO_SCOPE: &str = "repo";
const MAX_AVATAR: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubConfig {
    client_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GitHubBody<'a> {
    code: &'a str,
    redirect_uri: &'a str,
    code_verifier: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionBody {
    token: String,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsBody {
    url: String,
    token: String,
    expires_at: i64,
}

#[derive(Deserialize)]
struct TokenBody {
    token: String,
    scope: String,
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

    /// Somme's GitHub app, as the cloud names it.
    async fn github_config(&self) -> Result<GitHubConfig, CloudError> {
        let response = self
            .http
            .get(format!("{}/v1/auth/github", self.base))
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(Self::rejection(response).await);
        }
        response.json().await.map_err(unreachable)
    }
}

#[async_trait]
impl CloudAccounts for HttpCloudClient {
    async fn sign_in_with_github(&self) -> Result<CloudSession, CloudError> {
        let config = self.github_config().await?;
        let grant = github_login::sign_in(&config.client_id, None, |page: &str| open::that_detached(page)).await?;
        let response = self
            .http
            .post(format!("{}/v1/sessions/github", self.base))
            .json(&GitHubBody {
                code: &grant.code,
                redirect_uri: &grant.redirect_uri,
                code_verifier: &grant.code_verifier,
            })
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(Self::rejection(response).await);
        }
        let body: SessionBody = response.json().await.map_err(unreachable)?;
        Ok(CloudSession {
            token: Zeroizing::new(body.token),
            profile: CloudProfile {
                login: body.login,
                name: body.name,
                avatar_url: body.avatar_url,
            },
        })
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

    async fn github_access(&self, session: &CloudSession) -> Result<GitHubAccess, CloudError> {
        let config = self.github_config().await?;
        let grant =
            github_login::sign_in(&config.client_id, Some(REPO_SCOPE), |page: &str| open::that_detached(page)).await?;
        let response = self
            .http
            .post(format!("{}/v1/github/token", self.base))
            .bearer_auth(session.token.as_str())
            .json(&GitHubBody {
                code: &grant.code,
                redirect_uri: &grant.redirect_uri,
                code_verifier: &grant.code_verifier,
            })
            .send()
            .await
            .map_err(unreachable)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(CloudError::SessionExpired);
        }
        if !response.status().is_success() {
            return Err(Self::rejection(response).await);
        }
        let body: TokenBody = response.json().await.map_err(unreachable)?;
        Ok(GitHubAccess {
            token: Zeroizing::new(body.token),
            scope: body.scope,
        })
    }

    async fn github_access_page(&self) -> Result<String, CloudError> {
        let config = self.github_config().await?;
        let id = &config.client_id;
        // It goes into a URL; GitHub's ids are letters, digits and dots.
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return Err(CloudError::Rejected("Somme Cloud sent a strange GitHub app id".into()));
        }
        Ok(format!("https://github.com/settings/connections/applications/{id}"))
    }

    async fn avatar(&self, url: &str) -> Result<Vec<u8>, CloudError> {
        let mut url = reqwest::Url::parse(url).map_err(unreachable)?;
        if url.scheme() != "https" || url.host_str() != Some(AVATAR_HOST) {
            return Err(CloudError::Rejected("profile picture is not from GitHub".into()));
        }
        url.query_pairs_mut().append_pair("s", AVATAR_SIZE);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(unreachable)?;
        let bytes = response.bytes().await.map_err(unreachable)?;
        if bytes.len() > MAX_AVATAR {
            return Err(CloudError::Rejected("profile picture is too big".into()));
        }
        Ok(bytes.to_vec())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    token: String,
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// The non-secret part of a sign-in, kept beside the keychain item.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProfile {
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Keeps the sign-in in the system keychain. Reading the keychain can stop
/// and ask the user, so it happens once a run, and who is signed in is also
/// written to a plain file the app can read at start without asking.
pub struct KeyringSessionStore {
    keyring: KeyringStore,
    profile_file: PathBuf,
    /// The keychain's answer, once asked.
    read: StdMutex<Option<Option<CloudSession>>>,
}

impl KeyringSessionStore {
    pub fn new(profile_file: PathBuf) -> Self {
        Self {
            keyring: KeyringStore::new(SESSION_USER),
            profile_file,
            read: StdMutex::new(None),
        }
    }

    fn remember(&self, session: Option<CloudSession>) {
        let written = match &session {
            Some(session) => serde_json::to_string(&StoredProfile {
                login: session.profile.login.clone(),
                name: session.profile.name.clone(),
                avatar_url: session.profile.avatar_url.clone(),
            })
            .map_err(std::io::Error::other)
            .and_then(|raw| std::fs::write(&self.profile_file, raw)),
            None => match std::fs::remove_file(&self.profile_file) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(err) = written {
            tracing::warn!(%err, "could not update the saved profile");
        }
        *self.read.lock().unwrap_or_else(PoisonError::into_inner) = Some(session);
    }
}

impl SessionStore for KeyringSessionStore {
    fn load(&self) -> Option<CloudSession> {
        if let Some(session) = self.read.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            return session.clone();
        }
        let session = self.keyring.get_secret().and_then(|raw| {
            // Sessions saved before GitHub sign-in have no login and are dropped.
            let stored: StoredSession = serde_json::from_str(&raw).ok()?;
            Some(CloudSession {
                token: Zeroizing::new(stored.token),
                profile: CloudProfile {
                    login: stored.login,
                    name: stored.name,
                    avatar_url: stored.avatar_url,
                },
            })
        });
        self.remember(session.clone());
        session
    }

    fn profile(&self) -> Option<CloudProfile> {
        if let Some(session) = self.read.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            return session.as_ref().map(|s| s.profile.clone());
        }
        let raw = std::fs::read_to_string(&self.profile_file).ok()?;
        let stored: StoredProfile = serde_json::from_str(&raw).ok()?;
        Some(CloudProfile {
            login: stored.login,
            name: stored.name,
            avatar_url: stored.avatar_url,
        })
    }

    fn save(&self, session: &CloudSession) -> Result<(), GatewayError> {
        let raw = Zeroizing::new(
            serde_json::to_string(&StoredSession {
                token: session.token.to_string(),
                login: session.profile.login.clone(),
                name: session.profile.name.clone(),
                avatar_url: session.profile.avatar_url.clone(),
            })
            .map_err(|e| GatewayError::Io(e.to_string()))?,
        );
        self.keyring
            .set_secret(&raw)
            .map_err(|e| GatewayError::Io(e.to_string()))?;
        self.remember(Some(session.clone()));
        Ok(())
    }

    fn clear(&self) -> Result<(), GatewayError> {
        self.keyring
            .delete_secret()
            .map_err(|e| GatewayError::Io(e.to_string()))?;
        self.remember(None);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_is_signed_in_comes_from_the_file_until_the_keychain_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("account.json");
        let store = KeyringSessionStore::new(file.clone());
        assert_eq!(store.profile(), None);

        std::fs::write(&file, r#"{"login":"octocat","avatarUrl":"https://avatars.example.test/1"}"#).unwrap();
        let octocat = CloudProfile {
            login: "octocat".into(),
            name: None,
            avatar_url: Some("https://avatars.example.test/1".into()),
        };
        assert_eq!(store.profile(), Some(octocat));

        // Once the keychain has answered "nobody", the file goes too.
        store.remember(None);
        assert!(!file.exists());
        assert_eq!(store.profile(), None);
    }
}
