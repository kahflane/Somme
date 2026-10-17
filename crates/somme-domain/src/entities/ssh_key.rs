use std::str::FromStr;

use crate::entities::require;
use crate::error::DomainError;
use crate::values::{Id, Sealed, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAlgorithm {
    Ed25519,
    Ecdsa,
    Rsa,
}

impl FromStr for KeyAlgorithm {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ed25519" => Ok(Self::Ed25519),
            "ecdsa" => Ok(Self::Ecdsa),
            "rsa" => Ok(Self::Rsa),
            other => Err(DomainError::Invalid(format!("unsupported key algorithm: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshKey {
    pub id: Id,
    pub label: String,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub private_key: Sealed,
    pub passphrase: Option<Sealed>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl SshKey {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.label, "label")?;
        require(&self.public_key, "public key")
    }
}
