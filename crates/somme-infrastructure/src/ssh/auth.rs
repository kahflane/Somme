use std::sync::Arc;

use russh::MethodKind;
use russh::MethodSet;
use russh::client::{AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, decode_secret_key};
use somme_application::ports::{
    Endpoint, GatewayError, KeyCredential, KeyboardPrompt, PromptAnswer, PromptKind, Prompter,
};
use zeroize::Zeroizing;

use crate::ssh::connect::SshHandle;
use crate::ssh::connection_error;

const MAX_KEYBOARD_ROUNDS: usize = 8;

struct Attempt<'a> {
    handle: &'a mut SshHandle,
    hop: &'a Endpoint,
    prompter: &'a dyn Prompter,
    allowed: MethodSet,
}

pub async fn authenticate(
    handle: &mut SshHandle,
    hop: &Endpoint,
    prompter: &dyn Prompter,
) -> Result<(), GatewayError> {
    let allowed = match handle
        .authenticate_none(&hop.username)
        .await
        .map_err(connection_error)?
    {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    let mut attempt = Attempt {
        handle,
        hop,
        prompter,
        allowed,
    };
    if attempt.run().await? {
        Ok(())
    } else {
        Err(GatewayError::AuthFailed(format!("{}@{}", hop.username, hop.host)))
    }
}

impl Attempt<'_> {
    fn allows(&self, method: MethodKind) -> bool {
        self.allowed.contains(&method)
    }

    fn record(&mut self, result: AuthResult) -> bool {
        match result {
            AuthResult::Success => true,
            AuthResult::Failure {
                remaining_methods, ..
            } => {
                self.allowed = remaining_methods;
                false
            }
        }
    }

    async fn run(&mut self) -> Result<bool, GatewayError> {
        if self.allows(MethodKind::PublicKey) && self.try_keys().await? {
            return Ok(true);
        }
        if self.allows(MethodKind::PublicKey) && self.try_agent().await? {
            return Ok(true);
        }
        let mut password = self.hop.password.clone();
        let mut password_tried = false;
        if self.allows(MethodKind::Password)
            && (password.is_some() || !self.allows(MethodKind::KeyboardInteractive))
        {
            if password.is_none() {
                password = Some(self.ask_password().await?);
            }
            password_tried = true;
            if self.try_password(password.as_deref().map(String::as_str)).await? {
                return Ok(true);
            }
        }
        if self.allows(MethodKind::KeyboardInteractive)
            && self.try_keyboard(password.as_deref().map(String::as_str)).await?
        {
            return Ok(true);
        }
        if self.allows(MethodKind::Password) && !password_tried {
            let typed = self.ask_password().await?;
            return self.try_password(Some(&typed)).await;
        }
        Ok(false)
    }

    async fn try_keys(&mut self) -> Result<bool, GatewayError> {
        for credential in &self.hop.keys {
            let Some(key) = self.load_key(credential).await? else {
                continue;
            };
            let hash = self
                .handle
                .best_supported_rsa_hash()
                .await
                .map_err(connection_error)?
                .flatten();
            let result = self
                .handle
                .authenticate_publickey(&self.hop.username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(connection_error)?;
            if self.record(result) {
                return Ok(true);
            }
            if !self.allows(MethodKind::PublicKey) {
                break;
            }
        }
        Ok(false)
    }

    async fn load_key(&self, credential: &KeyCredential) -> Result<Option<PrivateKey>, GatewayError> {
        let passphrase = credential.passphrase.as_deref().map(String::as_str);
        match decode_secret_key(&credential.private_key, passphrase) {
            Ok(key) => return Ok(Some(key)),
            Err(russh::keys::Error::KeyIsEncrypted) if passphrase.is_none() => {}
            Err(err) => {
                tracing::warn!(%err, key = credential.label, "skipping unreadable key");
                return Ok(None);
            }
        }
        let answer = self
            .prompter
            .ask(
                &self.hop.title,
                PromptKind::Passphrase {
                    key_label: credential.label.clone(),
                },
            )
            .await;
        let PromptAnswer::Answers(answers) = answer else {
            return Ok(None);
        };
        let typed = Zeroizing::new(answers.into_iter().next().unwrap_or_default());
        match decode_secret_key(&credential.private_key, Some(&typed)) {
            Ok(key) => Ok(Some(key)),
            Err(err) => {
                tracing::warn!(%err, key = credential.label, "wrong passphrase");
                Ok(None)
            }
        }
    }

    #[cfg(unix)]
    async fn try_agent(&mut self) -> Result<bool, GatewayError> {
        use russh::keys::agent::AgentIdentity;
        use russh::keys::agent::client::AgentClient;

        let Ok(mut agent) = AgentClient::connect_env().await else {
            return Ok(false);
        };
        let Ok(identities) = agent.request_identities().await else {
            return Ok(false);
        };
        for identity in identities {
            let AgentIdentity::PublicKey { key, .. } = identity else {
                continue;
            };
            let hash = self
                .handle
                .best_supported_rsa_hash()
                .await
                .map_err(connection_error)?
                .flatten();
            match self
                .handle
                .authenticate_publickey_with(&self.hop.username, key, hash, &mut agent)
                .await
            {
                Ok(result) => {
                    if self.record(result) {
                        return Ok(true);
                    }
                    if !self.allows(MethodKind::PublicKey) {
                        break;
                    }
                }
                Err(err) => tracing::debug!(%err, "agent key failed"),
            }
        }
        Ok(false)
    }

    #[cfg(not(unix))]
    async fn try_agent(&mut self) -> Result<bool, GatewayError> {
        Ok(false)
    }

    async fn try_password(&mut self, password: Option<&str>) -> Result<bool, GatewayError> {
        let Some(password) = password else {
            return Ok(false);
        };
        let result = self
            .handle
            .authenticate_password(&self.hop.username, password)
            .await
            .map_err(connection_error)?;
        Ok(self.record(result))
    }

    async fn try_keyboard(&mut self, saved: Option<&str>) -> Result<bool, GatewayError> {
        let mut saved = saved;
        let mut response = self
            .handle
            .authenticate_keyboard_interactive_start(&self.hop.username, None::<String>)
            .await
            .map_err(connection_error)?;
        for _ in 0..MAX_KEYBOARD_ROUNDS {
            match response {
                KeyboardInteractiveAuthResponse::Success => return Ok(true),
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods, ..
                } => {
                    self.allowed = remaining_methods;
                    return Ok(false);
                }
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let answers = if prompts.is_empty() {
                        Vec::new()
                    } else if let (Some(password), [only]) = (saved, prompts.as_slice())
                        && !only.echo
                        && only.prompt.to_lowercase().contains("password")
                    {
                        saved = None;
                        vec![password.to_owned()]
                    } else {
                        let kind = PromptKind::KeyboardInteractive {
                            name,
                            instructions,
                            prompts: prompts
                                .into_iter()
                                .map(|p| KeyboardPrompt {
                                    text: p.prompt,
                                    echo: p.echo,
                                })
                                .collect(),
                        };
                        match self.prompter.ask(&self.hop.title, kind).await {
                            PromptAnswer::Answers(answers) => answers,
                            _ => return Err(GatewayError::Cancelled),
                        }
                    };
                    response = self
                        .handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                        .map_err(connection_error)?;
                }
            }
        }
        Ok(false)
    }

    async fn ask_password(&self) -> Result<Zeroizing<String>, GatewayError> {
        let kind = PromptKind::Password {
            username: self.hop.username.clone(),
            host: self.hop.host.clone(),
        };
        match self.prompter.ask(&self.hop.title, kind).await {
            PromptAnswer::Answers(answers) => {
                Ok(Zeroizing::new(answers.into_iter().next().unwrap_or_default()))
            }
            _ => Err(GatewayError::Cancelled),
        }
    }
}
