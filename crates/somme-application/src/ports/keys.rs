use somme_domain::KeyAlgorithm;
use zeroize::Zeroizing;

use crate::ports::GatewayError;

#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
}

pub struct GeneratedKey {
    pub private_key: Zeroizing<String>,
    pub info: KeyInfo,
}

pub trait KeyTool: Send + Sync {
    fn generate(&self, algorithm: KeyAlgorithm, comment: &str) -> Result<GeneratedKey, GatewayError>;
    fn inspect(&self, private_key: &str, passphrase: Option<&str>) -> Result<KeyInfo, GatewayError>;
}
