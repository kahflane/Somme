use crate::values::{Id, Timestamp};

#[derive(Debug, Clone, PartialEq)]
pub struct KnownHost {
    pub id: Id,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub public_key: String,
    pub created_at: Timestamp,
}

impl KnownHost {
    pub fn matches(&self, fingerprint: &str) -> bool {
        self.fingerprint == fingerprint
    }
}
