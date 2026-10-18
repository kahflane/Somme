use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use somme_domain::ForwardKind;
use zeroize::Zeroizing;

use crate::ports::{HostKeyVerifier, OutputSink, Prompter, TermSize, Terminal};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("cancelled")]
    Cancelled,
    #[error("authentication failed for {0}")]
    AuthFailed(String),
    #[error("host key was rejected")]
    HostKeyRejected,
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("session error: {0}")]
    Session(String),
    #[error("file transfer error: {0}")]
    Transfer(String),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("{0}")]
    Io(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("sudo needs your password")]
    SudoNeedsPassword,
    #[error("sudo did not accept the password")]
    SudoWrongPassword,
}

pub struct KeyCredential {
    pub label: String,
    pub private_key: Zeroizing<String>,
    pub passphrase: Option<Zeroizing<String>>,
}

pub struct Endpoint {
    pub title: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<Zeroizing<String>>,
    pub keys: Vec<KeyCredential>,
}

pub struct ConnectPlan {
    pub hops: Vec<Endpoint>,
}

impl ConnectPlan {
    pub fn target(&self) -> Option<&Endpoint> {
        self.hops.last()
    }
}

#[derive(Clone)]
pub struct ConnectContext {
    pub prompter: Arc<dyn Prompter>,
    pub verifier: Arc<dyn HostKeyVerifier>,
}

#[derive(Debug, Clone)]
pub struct ForwardSpec {
    pub kind: ForwardKind,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified_at: Option<i64>,
    pub permissions: Option<String>,
}

pub type StopNotifier = Box<dyn FnOnce(Option<String>) + Send>;

#[async_trait]
pub trait RemoteFs: Send + Sync {
    async fn home(&self) -> Result<String, GatewayError>;
    async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, GatewayError>;
    async fn mkdir(&self, path: &str) -> Result<(), GatewayError>;
    async fn rename(&self, from: &str, to: &str) -> Result<(), GatewayError>;
    async fn remove(&self, path: &str, is_dir: bool) -> Result<(), GatewayError>;
    async fn download(&self, remote: &str, local: &Path) -> Result<(), GatewayError>;
    async fn upload(&self, local: &Path, remote: &str) -> Result<(), GatewayError>;
    /// What is at `path`, following links.
    async fn stat(&self, path: &str) -> Result<RemoteEntry, GatewayError>;
    /// A file's bytes. Files over `limit` bytes are refused.
    async fn read(&self, path: &str, limit: u64) -> Result<Vec<u8>, GatewayError>;
    /// Replaces a file's contents, making the file when it is missing.
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), GatewayError>;
    /// [`RemoteFs::read`] as root, with sudo. `password` is the user's sudo
    /// password, for when sudo asks for one.
    async fn read_as_root(&self, path: &str, limit: u64, password: Option<&str>) -> Result<Vec<u8>, GatewayError> {
        let _ = (path, limit, password);
        Err(GatewayError::Session("sudo is not available here".into()))
    }
    /// [`RemoteFs::write`] as root, with sudo. The file keeps its owner and mode.
    async fn write_as_root(&self, path: &str, data: &[u8], password: Option<&str>) -> Result<(), GatewayError> {
        let _ = (path, data, password);
        Err(GatewayError::Session("sudo is not available here".into()))
    }
    async fn close(&self);
}

#[async_trait]
pub trait Tunnel: Send + Sync {
    async fn stop(&self);
}

#[async_trait]
pub trait SshGateway: Send + Sync {
    /// Starts the login shell. `marks` is the secret for Somme's shell hooks,
    /// loaded when the server's shell can take them.
    async fn shell(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
        size: TermSize,
        marks: Option<&str>,
        sink: Arc<dyn OutputSink>,
    ) -> Result<Arc<dyn Terminal>, GatewayError>;

    async fn sftp(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
    ) -> Result<Arc<dyn RemoteFs>, GatewayError>;

    async fn forward(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
        spec: ForwardSpec,
        on_stop: StopNotifier,
    ) -> Result<Arc<dyn Tunnel>, GatewayError>;
}
