use std::sync::{Mutex, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keyring::Entry;
use somme_application::ports::{CryptoError, GatewayError, KeyStore, MasterKey, SecretStore};
use zeroize::Zeroizing;

const SERVICE: &str = "app.somme.vault";
const MASTER_KEY_USER: &str = "master-key";
const GITHUB_TOKEN_USER: &str = "github-token";

pub struct KeyringStore {
    service: String,
    user: String,
}

impl KeyringStore {
    pub fn master_key() -> Self {
        Self::new(MASTER_KEY_USER)
    }

    pub fn new(user: &str) -> Self {
        Self {
            service: SERVICE.to_owned(),
            user: user.to_owned(),
        }
    }

    fn entry(&self) -> Result<Entry, CryptoError> {
        Entry::new(&self.service, &self.user).map_err(|e| CryptoError::KeyStore(e.to_string()))
    }

    pub fn get_secret(&self) -> Option<Zeroizing<String>> {
        self.entry().ok()?.get_password().ok().map(Zeroizing::new)
    }

    pub fn set_secret(&self, secret: &str) -> Result<(), CryptoError> {
        self.entry()?
            .set_password(secret)
            .map_err(|e| CryptoError::KeyStore(e.to_string()))
    }

    pub fn delete_secret(&self) -> Result<(), CryptoError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CryptoError::KeyStore(e.to_string())),
        }
    }
}

impl KeyStore for KeyringStore {
    fn load(&self) -> Option<MasterKey> {
        let encoded = self.get_secret()?;
        let bytes = Zeroizing::new(STANDARD.decode(encoded.as_bytes()).ok()?);
        let array: [u8; 32] = bytes.as_slice().try_into().ok()?;
        Some(MasterKey::from_bytes(array))
    }

    fn store(&self, key: &MasterKey) -> Result<(), CryptoError> {
        let encoded = Zeroizing::new(STANDARD.encode(key.as_bytes()));
        self.set_secret(&encoded)
    }

    fn clear(&self) -> Result<(), CryptoError> {
        self.delete_secret()
    }
}

/// One secret in the keychain, read once and then kept in memory, since a
/// keychain read can make the system ask the user.
pub struct KeyringSecret {
    keyring: KeyringStore,
    cached: Mutex<Option<Option<Zeroizing<String>>>>,
}

impl KeyringSecret {
    /// The GitHub token for pull requests and issues.
    pub fn github_token() -> Self {
        Self {
            keyring: KeyringStore::new(GITHUB_TOKEN_USER),
            cached: Mutex::new(None),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Option<Option<Zeroizing<String>>>> {
        self.cached.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SecretStore for KeyringSecret {
    fn load(&self) -> Option<Zeroizing<String>> {
        self.cache().get_or_insert_with(|| self.keyring.get_secret()).clone()
    }

    fn save(&self, secret: &str) -> Result<(), GatewayError> {
        self.keyring.set_secret(secret).map_err(|err| GatewayError::Io(err.to_string()))?;
        *self.cache() = Some(Some(Zeroizing::new(secret.to_owned())));
        Ok(())
    }

    fn clear(&self) -> Result<(), GatewayError> {
        self.keyring.delete_secret().map_err(|err| GatewayError::Io(err.to_string()))?;
        *self.cache() = Some(None);
        Ok(())
    }
}
