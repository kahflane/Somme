use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct KeyboardPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone)]
pub enum PromptKind {
    Password {
        username: String,
        host: String,
    },
    Passphrase {
        key_label: String,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<KeyboardPrompt>,
    },
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        previous_fingerprint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    Answers(Vec<String>),
    Accept,
    Cancel,
}

#[async_trait]
pub trait Prompter: Send + Sync {
    async fn ask(&self, title: &str, kind: PromptKind) -> PromptAnswer;
}

#[derive(Debug, Clone)]
pub struct ServerKey {
    pub key_type: String,
    pub fingerprint: String,
    pub public_key: String,
}

#[async_trait]
pub trait HostKeyVerifier: Send + Sync {
    async fn verify(&self, title: &str, host: &str, port: u16, key: &ServerKey) -> bool;
}
