use crate::entities::require;
use crate::error::DomainError;
use crate::values::{Id, Sealed, Timestamp};

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: Id,
    pub label: String,
    pub username: String,
    pub password: Option<Sealed>,
    pub key_id: Option<Id>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Identity {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.username, "username")
    }
}
