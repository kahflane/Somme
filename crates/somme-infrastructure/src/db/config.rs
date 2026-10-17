use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const LOCAL_DB: &str = "somme.db";
const CONFIG_FILE: &str = "sync.json";

#[derive(Debug, Clone)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn local_db(&self) -> PathBuf {
        self.root.join(LOCAL_DB)
    }

    pub fn synced_db(&self, url: &str) -> PathBuf {
        let slug: String = url
            .split("://")
            .last()
            .unwrap_or(url)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        self.root.join(format!("sync-{}.db", slug.trim_matches('-')))
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    pub url: Option<String>,
    pub enabled: bool,
}

impl SyncConfig {
    pub fn load(paths: &DataPaths) -> Self {
        std::fs::read_to_string(paths.config_file())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &DataPaths) -> std::io::Result<()> {
        let raw = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(paths.config_file(), raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_path_is_stable_per_url() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::new(dir.path()).unwrap();
        let a = paths.synced_db("libsql://db-org.turso.io");
        assert_eq!(a, paths.synced_db("libsql://db-org.turso.io"));
        assert!(a.ends_with("sync-db-org-turso-io.db"));
    }
}
