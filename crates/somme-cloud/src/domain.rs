use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, CloudError> {
        let email = raw.trim().to_lowercase();
        let valid = email.len() <= 254
            && email.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
            })
            && !email.chars().any(char::is_whitespace);
        if valid {
            Ok(Self(email))
        } else {
            Err(CloudError::Invalid("enter a valid email address".into()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn check_password(password: &str) -> Result<(), CloudError> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(CloudError::Invalid(format!(
            "password must have at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(CloudError::Invalid("password is too long".into()));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: Uuid,
    pub email: Email,
    pub password_hash: String,
    pub database_name: String,
    pub database_host: Option<String>,
    pub created_at: i64,
}

impl Account {
    pub fn database_name_for(id: Uuid) -> String {
        format!("somme-{}", &id.simple().to_string()[..24])
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub token: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCredentials {
    pub url: String,
    pub token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Invalid(String),
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error("wrong email or password")]
    BadCredentials,
    #[error("session expired, sign in again")]
    Unauthorized,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("database provisioning failed: {0}")]
    Provisioning(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_normalised_and_checked() {
        assert_eq!(Email::parse(" Me@Example.COM ").unwrap().as_str(), "me@example.com");
        assert!(Email::parse("nope").is_err());
        assert!(Email::parse("a@b").is_err());
        assert!(Email::parse("a b@c.d").is_err());
    }

    #[test]
    fn database_names_are_valid_turso_names() {
        let name = Account::database_name_for(Uuid::now_v7());
        assert!(name.len() <= 32);
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }
}
