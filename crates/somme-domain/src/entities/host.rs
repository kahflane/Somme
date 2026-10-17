use crate::entities::{require, require_port};
use crate::error::DomainError;
use crate::values::{Id, Sealed, Timestamp};

pub const DEFAULT_SSH_PORT: u16 = 22;

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub id: Id,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub group_id: Option<Id>,
    pub identity_id: Option<Id>,
    pub username: Option<String>,
    pub password: Option<Sealed>,
    pub key_id: Option<Id>,
    pub jump_host_id: Option<Id>,
    pub startup_snippet_id: Option<Id>,
    pub tags: Vec<String>,
    pub color: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Host {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.address, "address")?;
        require_port(self.port, "port")?;
        if self.address.chars().any(char::is_whitespace) {
            return Err(DomainError::Invalid("address cannot contain spaces".into()));
        }
        if self.jump_host_id == Some(self.id) {
            return Err(DomainError::Invalid("a host cannot jump through itself".into()));
        }
        Ok(())
    }

    pub fn display_name(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.address
        } else {
            &self.label
        }
    }

    pub fn detach(&mut self, id: Id) -> bool {
        let before = self.clone();
        for slot in [
            &mut self.group_id,
            &mut self.identity_id,
            &mut self.key_id,
            &mut self.jump_host_id,
            &mut self.startup_snippet_id,
        ] {
            if *slot == Some(id) {
                *slot = None;
            }
        }
        before != *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::new_id;

    fn host() -> Host {
        Host {
            id: new_id(),
            label: "web".into(),
            address: "10.0.0.1".into(),
            port: DEFAULT_SSH_PORT,
            group_id: None,
            identity_id: None,
            username: None,
            password: None,
            key_id: None,
            jump_host_id: None,
            startup_snippet_id: None,
            tags: vec![],
            color: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn rejects_empty_address() {
        let mut h = host();
        h.address = " ".into();
        assert_eq!(h.validate(), Err(DomainError::Required("address")));
    }

    #[test]
    fn rejects_self_jump() {
        let mut h = host();
        h.jump_host_id = Some(h.id);
        assert!(h.validate().is_err());
    }

    #[test]
    fn detach_clears_matching_links() {
        let mut h = host();
        let key = new_id();
        h.key_id = Some(key);
        assert!(h.detach(key));
        assert_eq!(h.key_id, None);
        assert!(!h.detach(key));
    }
}
