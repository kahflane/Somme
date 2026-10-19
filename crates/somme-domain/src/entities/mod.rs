mod automation;
mod group;
pub mod host;
mod identity;
mod known_host;
mod port_forward;
mod snippet;
mod ssh_key;
mod vault;

pub use automation::Automation;
pub use group::Group;
pub use host::Host;
pub use identity::Identity;
pub use known_host::KnownHost;
pub use port_forward::{ForwardKind, PortForward};
pub use snippet::Snippet;
pub use ssh_key::{KeyAlgorithm, SshKey};
pub use vault::VaultMeta;

use crate::error::DomainError;

pub(crate) fn require(value: &str, field: &'static str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        Err(DomainError::Required(field))
    } else {
        Ok(())
    }
}

pub(crate) fn require_port(port: u16, field: &'static str) -> Result<(), DomainError> {
    if port == 0 {
        Err(DomainError::Invalid(format!("{field} must be between 1 and 65535")))
    } else {
        Ok(())
    }
}
