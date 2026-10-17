mod auth;
mod connect;
mod forward;
mod handler;
mod sftp;
mod shell;

use std::sync::Arc;

use async_trait::async_trait;
use somme_application::ports::{
    ConnectContext, ConnectPlan, ForwardSpec, GatewayError, OutputSink, RemoteFs, SshGateway,
    StopNotifier, TermSize, Terminal, Tunnel,
};

use crate::ssh::connect::Connection;
use crate::ssh::handler::RemoteForwards;

#[derive(Debug, Default, Clone, Copy)]
pub struct RusshGateway;

#[async_trait]
impl SshGateway for RusshGateway {
    async fn shell(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
        size: TermSize,
        sink: Arc<dyn OutputSink>,
    ) -> Result<Arc<dyn Terminal>, GatewayError> {
        let conn = Connection::open(&plan, &ctx, RemoteForwards::default()).await?;
        Ok(Arc::new(shell::SshTerminal::start(conn, size, sink).await?))
    }

    async fn sftp(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
    ) -> Result<Arc<dyn RemoteFs>, GatewayError> {
        let conn = Connection::open(&plan, &ctx, RemoteForwards::default()).await?;
        Ok(Arc::new(sftp::SftpFs::start(conn).await?))
    }

    async fn forward(
        &self,
        plan: ConnectPlan,
        ctx: ConnectContext,
        spec: ForwardSpec,
        on_stop: StopNotifier,
    ) -> Result<Arc<dyn Tunnel>, GatewayError> {
        let remote_forwards = RemoteForwards::default();
        let conn = Connection::open(&plan, &ctx, remote_forwards.clone()).await?;
        Ok(Arc::new(
            forward::SshTunnel::start(conn, remote_forwards, spec, on_stop).await?,
        ))
    }
}

pub(crate) fn connection_error(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Connection(err.to_string())
}

pub(crate) fn session_error(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Session(err.to_string())
}
