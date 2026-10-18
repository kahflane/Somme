use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Somme Cloud account. Each one is a GitHub user with their own Turso database.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: Uuid,
    pub github_id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub database_name: String,
    pub database_host: Option<String>,
    pub created_at: i64,
}

impl Account {
    pub fn database_name_for(id: Uuid) -> String {
        format!("somme-{}", &id.simple().to_string()[..24])
    }
}

/// The GitHub user who just signed in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub token: String,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCredentials {
    pub url: String,
    pub token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubConfig {
    pub client_id: String,
}

/// A GitHub token for the app to keep. The server does not store it, and
/// it has no `Debug`, so it cannot end up in a log.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubToken {
    pub token: String,
    /// The scopes the user granted, comma separated.
    pub scope: String,
}

/// What the app gets back from GitHub after the user signs in in the browser.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubSignIn {
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Invalid(String),
    #[error("GitHub sign-in failed, try again")]
    BadCredentials,
    #[error("session expired, sign in again")]
    Unauthorized,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("database provisioning failed: {0}")]
    Provisioning(String),
    #[error("GitHub error: {0}")]
    GitHub(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names_are_valid_turso_names() {
        let name = Account::database_name_for(Uuid::now_v7());
        assert!(name.len() <= 32);
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }
}
