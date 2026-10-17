use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use somme_application::ports::{GatewayError, ImportedHost, SshConfigSource};
use ssh2_config::{ParseRule, SshConfig};

pub struct OpenSshConfigSource {
    path: Option<PathBuf>,
}

impl OpenSshConfigSource {
    pub fn user_default() -> Self {
        Self {
            path: dirs::home_dir().map(|h| h.join(".ssh").join("config")),
        }
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
}

fn is_concrete(pattern: &str) -> bool {
    !pattern.contains(['*', '?', '!'])
}

impl SshConfigSource for OpenSshConfigSource {
    fn load(&self) -> Result<Vec<ImportedHost>, GatewayError> {
        let Some(path) = self.path.as_ref().filter(|p| p.exists()) else {
            return Ok(Vec::new());
        };
        let file = File::open(path).map_err(|e| GatewayError::Io(e.to_string()))?;
        let config = SshConfig::default()
            .parse(&mut BufReader::new(file), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .map_err(|e| GatewayError::Io(e.to_string()))?;
        let aliases: Vec<String> = config
            .get_hosts()
            .iter()
            .flat_map(|h| h.pattern.iter())
            .filter(|clause| !clause.negated && is_concrete(&clause.pattern))
            .map(|clause| clause.pattern.clone())
            .collect();
        let mut hosts = Vec::new();
        for alias in aliases {
            let params = config.query(&alias);
            let private_keys = params
                .identity_file
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| {
                    let content = std::fs::read_to_string(&p).ok()?;
                    let label = p.file_name()?.to_string_lossy().into_owned();
                    Some((label, content))
                })
                .collect();
            hosts.push(ImportedHost {
                address: params.host_name.unwrap_or_else(|| alias.clone()),
                alias,
                port: params.port,
                username: params.user,
                private_keys,
            });
        }
        Ok(hosts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_concrete_hosts_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(
            &path,
            "Host web\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n\nHost *.internal\n  User x\n\nHost db\n  User postgres\n",
        )
        .unwrap();
        let hosts = OpenSshConfigSource::at(path).load().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "web");
        assert_eq!(hosts[0].address, "10.0.0.1");
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[0].username.as_deref(), Some("deploy"));
        assert_eq!(hosts[1].address, "db");
    }
}
