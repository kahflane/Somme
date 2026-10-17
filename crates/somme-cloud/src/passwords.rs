use std::sync::LazyLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::domain::CloudError;

pub fn hash(password: &str) -> Result<String, CloudError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| CloudError::Storage(e.to_string()))
}

pub fn verify(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| hash("somme-dummy-password").unwrap_or_default());

pub fn dummy_verify(password: &str) {
    let _ = verify(password, &DUMMY_HASH);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_verifies() {
        let stored = hash("correct horse battery").unwrap();
        assert!(verify("correct horse battery", &stored));
        assert!(!verify("wrong", &stored));
    }
}
