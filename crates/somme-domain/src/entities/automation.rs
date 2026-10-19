use crate::values::{Id, Sealed, Timestamp};

/// An automation, kept sealed with the vault: its JSON names the servers and
/// commands it runs, so only someone who can unlock may read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    pub id: Id,
    pub document: Sealed,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}
