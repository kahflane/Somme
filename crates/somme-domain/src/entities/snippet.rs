use crate::entities::require;
use crate::error::DomainError;
use crate::values::{Id, Timestamp};

#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub id: Id,
    pub label: String,
    pub script: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Snippet {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.label, "label")?;
        require(&self.script, "script")
    }
}
