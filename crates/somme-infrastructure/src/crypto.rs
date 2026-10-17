use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use somme_application::ports::{CryptoError, MasterKey, VaultCrypto};
use somme_domain::Sealed;
use zeroize::Zeroizing;

const PREFIX: &str = "v1:";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;

#[derive(Debug, Default, Clone, Copy)]
pub struct XChaChaVaultCrypto;

impl XChaChaVaultCrypto {
    fn cipher(key: &MasterKey) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(key.as_bytes().into())
    }
}

impl VaultCrypto for XChaChaVaultCrypto {
    fn new_salt(&self) -> String {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        STANDARD.encode(salt)
    }

    fn derive_key(&self, password: &str, salt: &str) -> Result<MasterKey, CryptoError> {
        let salt = STANDARD
            .decode(salt)
            .map_err(|e| CryptoError::Derive(e.to_string()))?;
        let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(32))
            .map_err(|e| CryptoError::Derive(e.to_string()))?;
        let mut out = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password.as_bytes(), &salt, &mut out)
            .map_err(|e| CryptoError::Derive(e.to_string()))?;
        let key = MasterKey::from_bytes(out);
        zeroize::Zeroize::zeroize(&mut out);
        Ok(key)
    }

    fn seal(&self, key: &MasterKey, plaintext: &str) -> Result<Sealed, CryptoError> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = Self::cipher(key)
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| CryptoError::Encrypt)?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(Sealed::new(format!("{PREFIX}{}", STANDARD.encode(blob))))
    }

    fn open(&self, key: &MasterKey, sealed: &Sealed) -> Result<Zeroizing<String>, CryptoError> {
        let encoded = sealed.as_str().strip_prefix(PREFIX).ok_or(CryptoError::Decrypt)?;
        let blob = STANDARD.decode(encoded).map_err(|_| CryptoError::Decrypt)?;
        if blob.len() < NONCE_LEN {
            return Err(CryptoError::Decrypt);
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plain = Zeroizing::new(
            Self::cipher(key)
                .decrypt(XNonce::from_slice(nonce), ciphertext)
                .map_err(|_| CryptoError::Decrypt)?,
        );
        let text = std::str::from_utf8(&plain).map_err(|_| CryptoError::Decrypt)?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open_round_trip() {
        let crypto = XChaChaVaultCrypto;
        let salt = crypto.new_salt();
        let key = crypto.derive_key("correct horse", &salt).unwrap();
        let sealed = crypto.seal(&key, "secret").unwrap();
        assert_eq!(crypto.open(&key, &sealed).unwrap().as_str(), "secret");
    }

    #[test]
    fn wrong_key_fails() {
        let crypto = XChaChaVaultCrypto;
        let salt = crypto.new_salt();
        let key = crypto.derive_key("one password", &salt).unwrap();
        let other = crypto.derive_key("two password", &salt).unwrap();
        let sealed = crypto.seal(&key, "secret").unwrap();
        assert!(crypto.open(&other, &sealed).is_err());
    }
}
