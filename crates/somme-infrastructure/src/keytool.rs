use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PrivateKey, decode_secret_key};
use somme_application::ports::{GatewayError, GeneratedKey, KeyInfo, KeyTool};
use somme_domain::KeyAlgorithm;

#[derive(Debug, Default, Clone, Copy)]
pub struct RusshKeyTool;

fn invalid(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::InvalidKey(err.to_string())
}

fn describe(key: &PrivateKey) -> Result<KeyInfo, GatewayError> {
    let public = key.public_key();
    Ok(KeyInfo {
        algorithm: public.algorithm().as_str().to_owned(),
        public_key: public.to_openssh().map_err(invalid)?,
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
    })
}

impl KeyTool for RusshKeyTool {
    fn generate(&self, algorithm: KeyAlgorithm, comment: &str) -> Result<GeneratedKey, GatewayError> {
        let algorithm = match algorithm {
            KeyAlgorithm::Ed25519 => Algorithm::Ed25519,
            KeyAlgorithm::Ecdsa => Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
            KeyAlgorithm::Rsa => Algorithm::Rsa { hash: None },
        };
        let mut key = PrivateKey::random(&mut rand::rng(), algorithm).map_err(invalid)?;
        key.set_comment(comment);
        let private_key = key.to_openssh(LineEnding::LF).map_err(invalid)?;
        Ok(GeneratedKey {
            private_key,
            info: describe(&key)?,
        })
    }

    fn inspect(&self, private_key: &str, passphrase: Option<&str>) -> Result<KeyInfo, GatewayError> {
        let key = decode_secret_key(private_key, passphrase).map_err(invalid)?;
        describe(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_can_be_read_back() {
        let tool = RusshKeyTool;
        for algorithm in [KeyAlgorithm::Ed25519, KeyAlgorithm::Ecdsa] {
            let generated = tool.generate(algorithm, "test").unwrap();
            let info = tool.inspect(&generated.private_key, None).unwrap();
            assert_eq!(info.fingerprint, generated.info.fingerprint);
            assert!(info.fingerprint.starts_with("SHA256:"));
            assert!(info.public_key.starts_with(&info.algorithm));
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(RusshKeyTool.inspect("not a key", None).is_err());
    }
}
