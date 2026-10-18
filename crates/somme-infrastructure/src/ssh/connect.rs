use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::{Disconnect, Preferred};
use somme_application::ports::{ConnectContext, ConnectPlan, Endpoint, GatewayError};

use crate::ssh::auth::authenticate;
use crate::ssh::connection_error;
use crate::ssh::handler::{ClientHandler, RemoteForwards};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX: usize = 3;

pub type SshHandle = Handle<ClientHandler>;

pub struct Connection {
    hops: Vec<Arc<SshHandle>>,
    server_id: Arc<OnceLock<String>>,
    banner: Arc<Mutex<String>>,
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: KEEPALIVE_MAX,
        nodelay: true,
        preferred: Preferred::default(),
        ..client::Config::default()
    })
}

impl Connection {
    pub async fn open(
        plan: &ConnectPlan,
        ctx: &ConnectContext,
        remote_forwards: RemoteForwards,
    ) -> Result<Self, GatewayError> {
        let config = config();
        let mut hops: Vec<Arc<SshHandle>> = Vec::with_capacity(plan.hops.len());
        let mut server_id = Arc::default();
        let mut banner = Arc::default();
        let last = plan.hops.len().saturating_sub(1);
        for (index, hop) in plan.hops.iter().enumerate() {
            let rejected = Arc::new(AtomicBool::new(false));
            server_id = Arc::new(OnceLock::new());
            banner = Arc::new(Mutex::new(String::new()));
            let handler = ClientHandler {
                title: hop.title.clone(),
                host: hop.host.clone(),
                port: hop.port,
                verifier: Arc::clone(&ctx.verifier),
                rejected: Arc::clone(&rejected),
                remote_forwards: if index == last {
                    remote_forwards.clone()
                } else {
                    RemoteForwards::default()
                },
                server_id: Arc::clone(&server_id),
                banner: Arc::clone(&banner),
            };
            let connected = tokio::time::timeout(
                CONNECT_TIMEOUT,
                dial(Arc::clone(&config), hops.last(), hop, handler),
            )
            .await
            .map_err(|_| GatewayError::Connection(format!("timed out connecting to {}", hop.host)))?;
            let mut handle = match connected {
                Ok(handle) => handle,
                Err(_) if rejected.load(Ordering::SeqCst) => return Err(GatewayError::HostKeyRejected),
                Err(err) => return Err(err),
            };
            authenticate(&mut handle, hop, ctx.prompter.as_ref()).await?;
            hops.push(Arc::new(handle));
        }
        if hops.is_empty() {
            return Err(GatewayError::Connection("nothing to connect to".into()));
        }
        Ok(Self { hops, server_id, banner })
    }

    pub fn target(&self) -> &Arc<SshHandle> {
        &self.hops[self.hops.len() - 1]
    }

    /// The last server's version line, such as `SSH-2.0-OpenSSH_9.6`.
    pub fn server_id(&self) -> Option<&str> {
        self.server_id.get().map(String::as_str)
    }

    /// The last server's pre-login banner, once. Empty when it sent none.
    pub fn take_banner(&self) -> String {
        self.banner.lock().map(|mut text| std::mem::take(&mut *text)).unwrap_or_default()
    }

    pub fn is_closed(&self) -> bool {
        self.hops.iter().any(|h| h.is_closed())
    }

    pub async fn disconnect(&self) {
        for handle in self.hops.iter().rev() {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "", "en")
                .await;
        }
    }
}

async fn dial(
    config: Arc<client::Config>,
    via: Option<&Arc<SshHandle>>,
    hop: &Endpoint,
    handler: ClientHandler,
) -> Result<SshHandle, GatewayError> {
    match via {
        None => client::connect(config, (hop.host.as_str(), hop.port), handler)
            .await
            .map_err(connection_error),
        Some(jump) => {
            let channel = jump
                .channel_open_direct_tcpip(hop.host.as_str(), u32::from(hop.port), "127.0.0.1", 0)
                .await
                .map_err(connection_error)?;
            client::connect_stream(config, channel.into_stream(), handler)
                .await
                .map_err(connection_error)
        }
    }
}
