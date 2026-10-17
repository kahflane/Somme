use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use somme_domain::{Sealed, VaultMeta, VaultRepository, now};
use zeroize::Zeroizing;

use crate::dto::VaultStatus;
use crate::error::{AppError, AppResult};
use crate::ports::{KeyStore, MasterKey, VaultCrypto};
use somme_domain::DomainError;

const CHECK_PLAINTEXT: &str = "somme-vault-check-v1";
const MIN_PASSWORD_LEN: usize = 8;

pub struct VaultService {
    repo: Arc<dyn VaultRepository>,
    crypto: Arc<dyn VaultCrypto>,
    keystore: Arc<dyn KeyStore>,
    key: RwLock<Option<MasterKey>>,
    auto_unlock_tried: AtomicBool,
}

impl VaultService {
    pub fn new(
        repo: Arc<dyn VaultRepository>,
        crypto: Arc<dyn VaultCrypto>,
        keystore: Arc<dyn KeyStore>,
    ) -> Self {
        Self {
            repo,
            crypto,
            keystore,
            key: RwLock::new(None),
            auto_unlock_tried: AtomicBool::new(false),
        }
    }

    pub async fn status(&self) -> AppResult<VaultStatus> {
        if !self.auto_unlock_tried.swap(true, Ordering::SeqCst) {
            self.auto_unlock().await?;
        }
        Ok(VaultStatus {
            initialized: self.repo.load().await?.is_some(),
            unlocked: self.is_unlocked(),
        })
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.read().map(|k| k.is_some()).unwrap_or(false)
    }

    pub async fn setup(&self, password: &str, remember: bool) -> AppResult<VaultStatus> {
        if self.repo.load().await?.is_some() {
            return Err(AppError::VaultAlreadyInitialized);
        }
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(DomainError::Invalid(format!(
                "master password must have at least {MIN_PASSWORD_LEN} characters"
            ))
            .into());
        }
        let salt = self.crypto.new_salt();
        let key = self.crypto.derive_key(password, &salt)?;
        let check = self.crypto.seal(&key, CHECK_PLAINTEXT)?;
        self.repo
            .store(&VaultMeta {
                salt,
                check,
                created_at: now(),
            })
            .await?;
        self.activate(key, remember)?;
        self.status().await
    }

    pub async fn unlock(&self, password: &str, remember: bool) -> AppResult<VaultStatus> {
        let meta = self.repo.load().await?.ok_or(AppError::VaultNotInitialized)?;
        let key = self.crypto.derive_key(password, &meta.salt)?;
        if !self.opens_check(&key, &meta) {
            return Err(AppError::WrongPassword);
        }
        self.activate(key, remember)?;
        self.status().await
    }

    async fn auto_unlock(&self) -> AppResult<()> {
        if self.is_unlocked() {
            return Ok(());
        }
        let Some(meta) = self.repo.load().await? else {
            return Ok(());
        };
        if let Some(key) = self.keystore.load() {
            if self.opens_check(&key, &meta) {
                self.set_key(Some(key));
            } else {
                self.keystore.clear()?;
            }
        }
        Ok(())
    }

    pub async fn lock(&self) -> AppResult<VaultStatus> {
        self.auto_unlock_tried.store(true, Ordering::SeqCst);
        self.set_key(None);
        self.keystore.clear()?;
        self.status().await
    }

    pub async fn revalidate(&self) -> AppResult<VaultStatus> {
        let still_valid = match (self.repo.load().await?, self.current_key()) {
            (Some(meta), Some(key)) => self.opens_check(&key, &meta),
            _ => false,
        };
        if !still_valid {
            self.set_key(None);
            self.keystore.clear()?;
        }
        self.status().await
    }

    pub fn seal(&self, plaintext: &str) -> AppResult<Sealed> {
        let key = self.current_key().ok_or(AppError::VaultLocked)?;
        Ok(self.crypto.seal(&key, plaintext)?)
    }

    pub fn open(&self, sealed: &Sealed) -> AppResult<Zeroizing<String>> {
        let key = self.current_key().ok_or(AppError::VaultLocked)?;
        Ok(self.crypto.open(&key, sealed)?)
    }

    fn opens_check(&self, key: &MasterKey, meta: &VaultMeta) -> bool {
        self.crypto
            .open(key, &meta.check)
            .map(|plain| plain.as_str() == CHECK_PLAINTEXT)
            .unwrap_or(false)
    }

    fn activate(&self, key: MasterKey, remember: bool) -> AppResult<()> {
        if remember {
            self.keystore.store(&key)?;
        } else {
            self.keystore.clear()?;
        }
        self.set_key(Some(key));
        Ok(())
    }

    fn current_key(&self) -> Option<MasterKey> {
        self.key.read().ok().and_then(|k| k.clone())
    }

    fn set_key(&self, key: Option<MasterKey>) {
        if let Ok(mut slot) = self.key.write() {
            *slot = key;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use somme_domain::RepositoryError;

    use super::*;
    use crate::ports::CryptoError;

    #[derive(Default)]
    struct MemoryVault(Mutex<Option<VaultMeta>>);

    #[async_trait]
    impl VaultRepository for MemoryVault {
        async fn load(&self) -> Result<Option<VaultMeta>, RepositoryError> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn store(&self, meta: &VaultMeta) -> Result<(), RepositoryError> {
            *self.0.lock().unwrap() = Some(meta.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryKeys(Mutex<Option<MasterKey>>);

    impl KeyStore for MemoryKeys {
        fn load(&self) -> Option<MasterKey> {
            self.0.lock().unwrap().clone()
        }

        fn store(&self, key: &MasterKey) -> Result<(), CryptoError> {
            *self.0.lock().unwrap() = Some(key.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), CryptoError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    struct TagCrypto;

    impl VaultCrypto for TagCrypto {
        fn new_salt(&self) -> String {
            "salt".into()
        }

        fn derive_key(&self, password: &str, salt: &str) -> Result<MasterKey, CryptoError> {
            let mut bytes = [0u8; 32];
            for (i, b) in password.bytes().chain(salt.bytes()).enumerate() {
                bytes[i % 32] ^= b;
            }
            Ok(MasterKey::from_bytes(bytes))
        }

        fn seal(&self, key: &MasterKey, plaintext: &str) -> Result<Sealed, CryptoError> {
            Ok(Sealed::new(format!("{:?}|{plaintext}", key.as_bytes())))
        }

        fn open(&self, key: &MasterKey, sealed: &Sealed) -> Result<Zeroizing<String>, CryptoError> {
            let (tag, plain) = sealed.as_str().split_once('|').ok_or(CryptoError::Decrypt)?;
            if tag == format!("{:?}", key.as_bytes()) {
                Ok(Zeroizing::new(plain.to_owned()))
            } else {
                Err(CryptoError::Decrypt)
            }
        }
    }

    fn service(repo: Arc<MemoryVault>, keys: Arc<MemoryKeys>) -> VaultService {
        VaultService::new(repo, Arc::new(TagCrypto), keys)
    }

    #[tokio::test]
    async fn setup_unlock_lock_cycle() {
        let repo = Arc::new(MemoryVault::default());
        let keys = Arc::new(MemoryKeys::default());
        let vault = service(repo.clone(), keys.clone());
        let status = vault.status().await.unwrap();
        assert!(!status.initialized && !status.unlocked);
        assert!(vault.setup("short", false).await.is_err());
        assert!(vault.setup("long password", false).await.unwrap().unlocked);
        assert!(matches!(vault.setup("long password", false).await, Err(AppError::VaultAlreadyInitialized)));
        let sealed = vault.seal("secret").unwrap();
        assert_eq!(vault.open(&sealed).unwrap().as_str(), "secret");
        assert!(!vault.lock().await.unwrap().unlocked);
        assert!(matches!(vault.seal("x"), Err(AppError::VaultLocked)));
        assert!(matches!(vault.unlock("wrong password", false).await, Err(AppError::WrongPassword)));
        assert!(vault.unlock("long password", false).await.unwrap().unlocked);
        assert!(keys.load().is_none());
    }

    #[tokio::test]
    async fn remembered_key_unlocks_on_first_status() {
        let repo = Arc::new(MemoryVault::default());
        let keys = Arc::new(MemoryKeys::default());
        service(repo.clone(), keys.clone())
            .setup("long password", true)
            .await
            .unwrap();
        assert!(keys.load().is_some());
        let restarted = service(repo, keys);
        assert!(restarted.status().await.unwrap().unlocked);
    }
}
