use std::fmt;
use std::str::FromStr;

use crate::entities::{require, require_port};
use crate::error::DomainError;
use crate::values::{Id, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

impl ForwardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Dynamic => "dynamic",
        }
    }
}

impl fmt::Display for ForwardKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ForwardKind {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            "dynamic" => Ok(Self::Dynamic),
            other => Err(DomainError::Invalid(format!("unknown forward kind: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortForward {
    pub id: Id,
    pub label: String,
    pub host_id: Id,
    pub kind: ForwardKind,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl PortForward {
    pub fn validate(&self) -> Result<(), DomainError> {
        require(&self.bind_address, "bind address")?;
        require_port(self.bind_port, "bind port")?;
        if self.kind == ForwardKind::Dynamic {
            return Ok(());
        }
        match (&self.target_host, self.target_port) {
            (Some(host), Some(port)) => {
                require(host, "target host")?;
                require_port(port, "target port")
            }
            _ => Err(DomainError::Required("target host and port")),
        }
    }

    pub fn target(&self) -> Option<(&str, u16)> {
        Some((self.target_host.as_deref()?, self.target_port?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::new_id;

    fn rule(kind: ForwardKind) -> PortForward {
        PortForward {
            id: new_id(),
            label: String::new(),
            host_id: new_id(),
            kind,
            bind_address: "127.0.0.1".into(),
            bind_port: 8080,
            target_host: None,
            target_port: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn dynamic_needs_no_target() {
        assert!(rule(ForwardKind::Dynamic).validate().is_ok());
    }

    #[test]
    fn local_needs_target() {
        let mut r = rule(ForwardKind::Local);
        assert!(r.validate().is_err());
        r.target_host = Some("db".into());
        r.target_port = Some(5432);
        assert!(r.validate().is_ok());
    }

    #[test]
    fn kind_round_trips() {
        for kind in [ForwardKind::Local, ForwardKind::Remote, ForwardKind::Dynamic] {
            assert_eq!(kind.as_str().parse::<ForwardKind>(), Ok(kind));
        }
    }
}
