use somme_domain::Sealed;

use crate::dto::SecretUpdate;
use crate::error::AppResult;
use crate::services::VaultService;

pub fn apply_secret(
    vault: &VaultService,
    current: Option<Sealed>,
    update: SecretUpdate,
) -> AppResult<Option<Sealed>> {
    match update {
        SecretUpdate::Keep => Ok(current),
        SecretUpdate::Clear => Ok(None),
        SecretUpdate::Set(value) if value.is_empty() => Ok(None),
        SecretUpdate::Set(value) => vault.seal(&value).map(Some),
    }
}

pub fn clean(value: &str) -> String {
    value.trim().to_owned()
}

pub fn clean_opt(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}
