//! Pinned terminal tabs, kept in the data folder so they come back at the
//! next start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use somme_domain::Id;

const FILE: &str = "pinned-tabs.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pinned {
    /// A shell on this computer, reopened in the folder it was last in.
    Local { dir: Option<PathBuf> },
    /// A saved host, connected again once the vault is open.
    Ssh { host: Id },
}

pub fn load(data_dir: &Path) -> Vec<Pinned> {
    std::fs::read_to_string(data_dir.join(FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(data_dir: &Path, pinned: &[Pinned]) {
    let result = serde_json::to_string_pretty(pinned)
        .map_err(std::io::Error::other)
        .and_then(|raw| std::fs::write(data_dir.join(FILE), raw));
    if let Err(err) = result {
        tracing::warn!(%err, "could not save the pinned tabs");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_tabs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
        let pinned = vec![
            Pinned::Local { dir: Some(PathBuf::from("/srv/app")) },
            Pinned::Ssh { host: Id::nil() },
            Pinned::Local { dir: None },
        ];
        save(dir.path(), &pinned);
        assert_eq!(load(dir.path()), pinned);
    }
}
