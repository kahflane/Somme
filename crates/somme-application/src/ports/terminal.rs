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
}

pub trait PtyGateway: Send + Sync {
    fn spawn(&self, size: TermSize, sink: Arc<dyn OutputSink>) -> Result<Arc<dyn Terminal>, GatewayError>;
}
