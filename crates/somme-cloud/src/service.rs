use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{Account, CloudError, Email, SessionView, SyncCredentials, check_password};
use crate::passwords;
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

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, CloudError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CloudError::Storage(e.to_string()))
}

pub struct AccountService {
    store: AccountStore,
    provisioner: Arc<dyn Provisioner>,
    token_ttl_days: u32,
}

impl AccountService {
    pub fn new(store: AccountStore, provisioner: Arc<dyn Provisioner>, token_ttl_days: u32) -> Self {
        Self {
            store,
            provisioner,
            token_ttl_days,
        }
    }

    pub async fn register(&self, email: &str, password: &str) -> Result<SessionView, CloudError> {
        let email = Email::parse(email)?;
        check_password(password)?;
        if self.store.find_by_email(&email).await?.is_some() {
            return Err(CloudError::EmailTaken);
        }
        let owned = password.to_owned();
        let password_hash = blocking(move || passwords::hash(&owned)).await??;
        let id = Uuid::now_v7();
        let account = Account {
            id,
            email,
            password_hash,
            database_name: Account::database_name_for(id),
            database_host: None,
            created_at: now(),
        };
        self.store.insert_account(&account).await?;
        self.open_session(&account).await
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<SessionView, CloudError> {
        let email = Email::parse(email).map_err(|_| CloudError::BadCredentials)?;
        let account = self.store.find_by_email(&email).await?;
        let owned = password.to_owned();
        let Some(account) = account else {
            blocking(move || passwords::dummy_verify(&owned)).await?;
            return Err(CloudError::BadCredentials);
        };
        let stored = account.password_hash.clone();
        if !blocking(move || passwords::verify(&owned, &stored)).await? {
            return Err(CloudError::BadCredentials);
        }
        self.open_session(&account).await
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
            email: account.email.as_str().to_owned(),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

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

    pub async fn service() -> (tempfile::TempDir, AccountService, Arc<FakeProvisioner>) {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path()).await.unwrap();
        let fake = Arc::new(FakeProvisioner::default());
        (dir, AccountService::new(store, fake.clone(), 7), fake)
    }

    #[tokio::test]
    async fn register_login_credentials_logout() {
        let (_dir, svc, fake) = service().await;
        let session = svc.register("Dev@Example.com", "long enough pw").await.unwrap();
        assert_eq!(session.email, "dev@example.com");
        assert!(matches!(
            svc.register("dev@example.com", "long enough pw").await,
            Err(CloudError::EmailTaken)
        ));
        assert!(matches!(
            svc.login("dev@example.com", "wrong password!").await,
            Err(CloudError::BadCredentials)
        ));
        let again = svc.login("dev@example.com", "long enough pw").await.unwrap();
        let first = svc.credentials(&session.token).await.unwrap();
        let second = svc.credentials(&again.token).await.unwrap();
        assert_eq!(first.url, second.url);
        assert!(first.url.starts_with("libsql://somme-"));
        assert_eq!(fake.created.lock().unwrap().len(), 1);
        svc.logout(&session.token).await.unwrap();
        assert!(matches!(svc.credentials(&session.token).await, Err(CloudError::Unauthorized)));
        assert!(svc.credentials(&again.token).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_weak_input() {
        let (_dir, svc, _) = service().await;
        assert!(matches!(svc.register("x@y.z", "short").await, Err(CloudError::Invalid(_))));
        assert!(matches!(svc.login("unknown@y.z", "whatever123").await, Err(CloudError::BadCredentials)));
        assert!(matches!(svc.credentials("bogus").await, Err(CloudError::Unauthorized)));
    }
}
