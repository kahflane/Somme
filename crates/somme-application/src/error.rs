use somme_domain::{DomainError, RepositoryError};

use crate::ports::{CloudError, CryptoError, GatewayError};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    #[error(transparent)]
    Cloud(#[from] CloudError),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("the vault is locked")]
    VaultLocked,
    #[error("the vault is not set up yet")]
    VaultNotInitialized,
    #[error("the vault is already set up")]
    VaultAlreadyInitialized,
    #[error("wrong master password")]
    WrongPassword,
    #[error("{0}")]
    Conflict(String),
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Domain(_) => "validation",
            Self::Repository(_) => "storage",
            Self::Crypto(_) => "crypto",
            Self::Gateway(GatewayError::Cancelled) => "cancelled",
            Self::Gateway(GatewayError::AuthFailed(_)) => "auth",
            Self::Gateway(GatewayError::HostKeyRejected) => "host_key",
            Self::Gateway(_) => "connection",
            Self::Cloud(CloudError::SessionExpired) => "signed_out",
            Self::Cloud(CloudError::Rejected(_)) => "account",
            Self::Cloud(CloudError::Unreachable(_)) => "offline",
            Self::NotFound(_) => "not_found",
            Self::VaultLocked => "vault_locked",
            Self::VaultNotInitialized => "vault_not_initialized",
            Self::VaultAlreadyInitialized => "vault_initialized",
            Self::WrongPassword => "wrong_password",
            Self::Conflict(_) => "conflict",
        }
    }
}
