use crate::ports::GatewayError;

#[derive(Debug, Clone)]
pub struct ImportedHost {
    pub alias: String,
    pub address: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub private_keys: Vec<(String, String)>,
}

pub trait SshConfigSource: Send + Sync {
    fn load(&self) -> Result<Vec<ImportedHost>, GatewayError>;
}
