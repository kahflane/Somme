use crate::values::{Sealed, Timestamp};

#[derive(Debug, Clone, PartialEq)]
pub struct VaultMeta {
    pub salt: String,
    pub check: Sealed,
    pub created_at: Timestamp,
}
