use somme_domain::Sealed;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const MASTER_KEY_LEN: usize = 32;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; MASTER_KEY_LEN]);

impl MasterKey {
    pub fn from_bytes(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(***)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key derivation failed: {0}")]
    Derive(String),
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("key store error: {0}")]
    KeyStore(String),
}

pub trait VaultCrypto: Send + Sync {
    fn new_salt(&self) -> String;
    fn derive_key(&self, password: &str, salt: &str) -> Result<MasterKey, CryptoError>;
    fn seal(&self, key: &MasterKey, plaintext: &str) -> Result<Sealed, CryptoError>;
    fn open(&self, key: &MasterKey, sealed: &Sealed) -> Result<Zeroizing<String>, CryptoError>;
}

pub trait KeyStore: Send + Sync {
    fn load(&self) -> Option<MasterKey>;
    fn store(&self, key: &MasterKey) -> Result<(), CryptoError>;
    fn clear(&self) -> Result<(), CryptoError>;
}
