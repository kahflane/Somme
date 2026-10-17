use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keyring::Entry;
use somme_application::ports::{CryptoError, KeyStore, MasterKey};
use zeroize::Zeroizing;

const SERVICE: &str = "app.somme.vault";
const MASTER_KEY_USER: &str = "master-key";

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
