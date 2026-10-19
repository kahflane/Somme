//! Vault values for automations, like `{{vault.DB_PASSWORD}}`: kept sealed
//! with the master password, hidden in run logs and never sent to AI.

use std::collections::BTreeMap;

use somme_domain::Sealed;

use crate::services::Services;

const PREFERENCE: &str = "automations.vault";

/// Shorter values would hide too much of every log.
pub const SHORTEST: usize = 4;

pub const LOCKED: &str = "Somme is locked. Unlock it with your master password, then run it again.";

/// Every value, opened.
pub async fn load(services: &Services) -> Result<BTreeMap<String, String>, String> {
    let stored = services.preferences.get(PREFERENCE).await.map_err(|err| format!("Could not read the vault values: {err}"))?;
    let Some(stored) = stored.filter(|stored| !stored.is_empty()) else { return Ok(BTreeMap::new()) };
    if !services.vault.is_unlocked() {
        return Err(LOCKED.into());
    }
    let opened = services.vault.open(&Sealed::new(stored)).map_err(|err| format!("Could not open the vault values: {err}"))?;
    serde_json::from_str(&opened).map_err(|err| format!("Could not read the vault values: {err}"))
}

pub async fn save(services: &Services, values: &BTreeMap<String, String>) -> Result<(), String> {
    let stored = if values.is_empty() {
        String::new()
    } else {
        let text = serde_json::to_string(values).map_err(|err| err.to_string())?;
        services.vault.seal(&text).map_err(|err| format!("Could not seal the vault values: {err}"))?.into_inner()
    };
    services.preferences.set(PREFERENCE, &stored).await.map_err(|err| format!("Could not save the vault values: {err}"))
}

/// Why a name or value can't be kept, if it can't.
pub fn check(name: &str, value: &str) -> Option<String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Some("A name uses letters, digits and _ only, like DB_PASSWORD.".into());
    }
    if value.chars().count() < SHORTEST {
        return Some(format!("A value needs at least {SHORTEST} characters, so hiding it in logs hides only it."));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_values_are_checked() {
        assert!(check("DB_PASSWORD", "hunter22").is_none());
        assert!(check("db password", "hunter22").is_some());
        assert!(check("TOKEN", "abc").is_some());
    }
}
