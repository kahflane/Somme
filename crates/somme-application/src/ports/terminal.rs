use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::ports::GatewayError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

pub trait OutputSink: Send + Sync {
    fn data(&self, bytes: &[u8]);
    fn closed(&self, reason: Option<String>);
}

#[async_trait]
pub trait Terminal: Send + Sync {
    async fn write(&self, data: &[u8]) -> Result<(), GatewayError>;
    async fn resize(&self, size: TermSize) -> Result<(), GatewayError>;
    async fn close(&self);

    /// The folder the shell is in now. Only local terminals know it.
    fn cwd(&self) -> Option<PathBuf> {
        None
    }
}

/// How to start a local shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellOptions<'a> {
    /// The folder to start in. Unset or gone means the home folder.
    pub dir: Option<&'a Path>,
    /// Load Somme's shell hooks, which mark where each prompt and command
    /// starts. Each mark carries this secret, so program output cannot fake one.
    pub marks: Option<&'a str>,
}

pub trait PtyGateway: Send + Sync {
    /// Starts the user's shell.
    fn spawn(
        &self,
        size: TermSize,
        options: ShellOptions<'_>,
        sink: Arc<dyn OutputSink>,
    ) -> Result<Arc<dyn Terminal>, GatewayError>;
}
