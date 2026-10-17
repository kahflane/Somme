use crate::entities::require;
use crate::error::DomainError;
use crate::values::{Id, Timestamp};

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: Id,
    pub name: String,
    pub parent_id: Option<Id>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Group {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.name, "name")?;
        if self.parent_id == Some(self.id) {
            return Err(DomainError::Invalid("a group cannot be its own parent".into()));
        }
        Ok(())
    }
}
