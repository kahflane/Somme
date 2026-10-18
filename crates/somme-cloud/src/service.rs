use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::domain::{Account, CloudError, GitHubConfig, GitHubSignIn, GitHubToken, SessionView, SyncCredentials};
use crate::github::GitHub;
use crate::platform::Provisioner;
use crate::store::AccountStore;

const SESSION_TTL_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const TOKEN_SAFETY_MS: i64 = 60 * 60 * 1000;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub struct AccountService {
    store: AccountStore,
    provisioner: Arc<dyn Provisioner>,
    github: Arc<dyn GitHub>,
    token_ttl_days: u32,
}

fn check(request: &GitHubSignIn) -> Result<(), CloudError> {
    if request.code.is_empty() || request.code_verifier.is_empty() || !is_loopback_callback(&request.redirect_uri) {
        return Err(CloudError::Invalid("bad sign-in request".into()));
    }
    Ok(())
}

/// The app listens on the loopback address, at a port of its choosing.
fn is_loopback_callback(uri: &str) -> bool {
    uri.strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/callback"))
        .is_some_and(|port| port.parse::<u16>().is_ok_and(|port| port != 0))
}

impl AccountService {
    pub fn new(
        store: AccountStore,
        provisioner: Arc<dyn Provisioner>,
        github: Arc<dyn GitHub>,
        token_ttl_days: u32,
    ) -> Self {
        Self {
            store,
            provisioner,
            github,
            token_ttl_days,
        }
    }

    pub fn github_config(&self) -> GitHubConfig {
        GitHubConfig {
            client_id: self.github.client_id().to_owned(),
        }
    }

    pub async fn sign_in_with_github(&self, request: &GitHubSignIn) -> Result<SessionView, CloudError> {
        check(request)?;
        let user = self
            .github
            .user(&request.code, &request.redirect_uri, &request.code_verifier)
            .await?;
        let account = self.store.sign_in(&user, now()).await?;
        self.open_session(&account).await
    }

    /// Trades a code the signed-in user approved in the browser for a GitHub
    /// token the app keeps, to show their pull requests and issues. It must
    /// be the same GitHub user, so a code from someone else is refused.
    pub async fn github_token(&self, session: &str, request: &GitHubSignIn) -> Result<GitHubToken, CloudError> {
        let account = self.authenticate(session).await?;
        check(request)?;
        let access = self
            .github
            .access(&request.code, &request.redirect_uri, &request.code_verifier)
            .await?;
        if access.user.id != account.github_id {
            self.github.revoke(&access.token).await;
            return Err(CloudError::Invalid(format!(
                "That is GitHub user {}, but Somme is signed in as {}.",
                access.user.login, account.login
            )));
        }
        Ok(GitHubToken {
            token: access.token,
            scope: access.scope,
        })
    }

    pub async fn logout(&self, token: &str) -> Result<(), CloudError> {
        self.store.delete_session(&hash_token(token)).await
    }

    pub async fn credentials(&self, token: &str) -> Result<SyncCredentials, CloudError> {
        let account = self.authenticate(token).await?;
        let host = match account.database_host {
            Some(host) => host,
            None => {
                let host = self.provisioner.ensure_database(&account.database_name).await?;
                self.store.set_database_host(account.id, &host).await?;
                host
            }
        };
        let token = self
            .provisioner
            .mint_token(&account.database_name, self.token_ttl_days)
            .await?;
        Ok(SyncCredentials {
            url: format!("libsql://{host}"),
            token,
            expires_at: now() + i64::from(self.token_ttl_days) * DAY_MS - TOKEN_SAFETY_MS,
        })
    }

    async fn authenticate(&self, token: &str) -> Result<Account, CloudError> {
        let hashed = hash_token(token);
        let (account_id, expires_at) = self
            .store
            .find_session(&hashed)
            .await?
            .ok_or(CloudError::Unauthorized)?;
        if expires_at <= now() {
            self.store.delete_session(&hashed).await?;
            return Err(CloudError::Unauthorized);
        }
        self.store
            .find_by_id(account_id)
            .await?
            .ok_or(CloudError::Unauthorized)
    }

    async fn open_session(&self, account: &Account) -> Result<SessionView, CloudError> {
        let token = new_token();
        let stamp = now();
        self.store
            .insert_session(&hash_token(&token), account.id, stamp + SESSION_TTL_MS, stamp)
            .await?;
        Ok(SessionView {
            token,
            login: account.login.clone(),
            name: account.name.clone(),
            avatar_url: account.avatar_url.clone(),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::domain::GitHubUser;
    use crate::github::GitHubAccess;

    #[derive(Default)]
    pub struct FakeProvisioner {
        pub created: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Provisioner for FakeProvisioner {
        async fn ensure_database(&self, name: &str) -> Result<String, CloudError> {
            self.created.lock().unwrap().push(name.to_owned());
            Ok(format!("{name}-org.turso.io"))
        }

        async fn mint_token(&self, name: &str, _ttl_days: u32) -> Result<String, CloudError> {
            Ok(format!("jwt-for-{name}"))
        }
    }

    #[derive(Default)]
    pub struct FakeGitHub {
        pub revoked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl GitHub for FakeGitHub {
        fn client_id(&self) -> &str {
            "fake-client"
        }

        /// Code `user-N` signs in GitHub user N. Any other code is refused.
        async fn user(&self, code: &str, _: &str, _: &str) -> Result<GitHubUser, CloudError> {
            let id = code.strip_prefix("user-").and_then(|id| id.parse().ok()).ok_or(CloudError::BadCredentials)?;
            Ok(GitHubUser {
                id,
                login: format!("user{id}"),
                name: None,
                avatar_url: Some(format!("https://avatars.githubusercontent.com/u/{id}")),
            })
        }

        async fn access(&self, code: &str, redirect_uri: &str, verifier: &str) -> Result<GitHubAccess, CloudError> {
            let user = self.user(code, redirect_uri, verifier).await?;
            Ok(GitHubAccess {
                token: format!("gho_for_{}", user.login),
                user,
                scope: "repo".into(),
            })
        }

        async fn revoke(&self, token: &str) {
            self.revoked.lock().unwrap().push(token.to_owned());
        }
    }

    pub fn sign_in(code: &str) -> GitHubSignIn {
        GitHubSignIn {
            code: code.into(),
            redirect_uri: "http://127.0.0.1:50123/callback".into(),
            code_verifier: "verifier".into(),
        }
    }

    pub async fn service() -> (tempfile::TempDir, AccountService, Arc<FakeProvisioner>) {
        let (dir, svc, fake, _) = service_with_github().await;
        (dir, svc, fake)
    }

    pub async fn service_with_github() -> (tempfile::TempDir, AccountService, Arc<FakeProvisioner>, Arc<FakeGitHub>) {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path()).await.unwrap();
        let fake = Arc::new(FakeProvisioner::default());
        let github = Arc::new(FakeGitHub::default());
        (dir, AccountService::new(store, fake.clone(), github.clone(), 7), fake, github)
    }

    #[tokio::test]
    async fn hands_out_a_github_token_only_for_the_signed_in_user() {
        let (_dir, svc, _, github) = service_with_github().await;
        let session = svc.sign_in_with_github(&sign_in("user-4")).await.unwrap();
        let token = svc.github_token(&session.token, &sign_in("user-4")).await.unwrap();
        assert_eq!(token.token, "gho_for_user4");
        assert_eq!(token.scope, "repo");
        assert!(github.revoked.lock().unwrap().is_empty());

        // The token type can't be printed, so no unwrap_err.
        let err = svc.github_token(&session.token, &sign_in("user-5")).await.err();
        assert!(matches!(err, Some(CloudError::Invalid(_))), "another GitHub user");
        assert_eq!(*github.revoked.lock().unwrap(), ["gho_for_user5"]);
        assert!(matches!(svc.github_token("bogus", &sign_in("user-4")).await, Err(CloudError::Unauthorized)));
    }

    #[tokio::test]
    async fn sign_in_credentials_logout() {
        let (_dir, svc, fake) = service().await;
        let session = svc.sign_in_with_github(&sign_in("user-7")).await.unwrap();
        assert_eq!(session.login, "user7");
        let again = svc.sign_in_with_github(&sign_in("user-7")).await.unwrap();
        let first = svc.credentials(&session.token).await.unwrap();
        let second = svc.credentials(&again.token).await.unwrap();
        assert_eq!(first.url, second.url);
        assert!(first.url.starts_with("libsql://somme-"));
        assert_eq!(fake.created.lock().unwrap().len(), 1);
        let other = svc.sign_in_with_github(&sign_in("user-8")).await.unwrap();
        assert_ne!(svc.credentials(&other.token).await.unwrap().url, first.url);
        svc.logout(&session.token).await.unwrap();
        assert!(matches!(svc.credentials(&session.token).await, Err(CloudError::Unauthorized)));
        assert!(svc.credentials(&again.token).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_bad_sign_ins() {
        let (_dir, svc, _) = service().await;
        assert!(matches!(svc.sign_in_with_github(&sign_in("stolen")).await, Err(CloudError::BadCredentials)));
        let mut elsewhere = sign_in("user-1");
        elsewhere.redirect_uri = "https://evil.example/callback".into();
        assert!(matches!(svc.sign_in_with_github(&elsewhere).await, Err(CloudError::Invalid(_))));
        assert!(matches!(svc.credentials("bogus").await, Err(CloudError::Unauthorized)));
    }

    #[test]
    fn only_loopback_callbacks_are_allowed() {
        assert!(is_loopback_callback("http://127.0.0.1:8123/callback"));
        assert!(!is_loopback_callback("http://127.0.0.1:0/callback"));
        assert!(!is_loopback_callback("http://127.0.0.1.evil.example:80/callback"));
        assert!(!is_loopback_callback("http://localhost:8123/callback"));
        assert!(!is_loopback_callback("http://127.0.0.1:8123/callback/x"));
    }
}
