use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use fast_socks5::server::Socks5ServerProtocol;
use fast_socks5::util::target_addr::TargetAddr;
use fast_socks5::{ReplyError, Socks5Command};
use somme_application::ports::{ForwardSpec, GatewayError, StopNotifier, Tunnel};
use somme_domain::ForwardKind;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::ssh::connect::Connection;
use crate::ssh::connection_error;
use crate::ssh::handler::RemoteForwards;

const HEALTH_INTERVAL: Duration = Duration::from_secs(3);

pub struct SshTunnel {
    conn: Arc<Connection>,
    cancel: CancellationToken,
    remote: Option<(String, u32)>,
}

impl SshTunnel {
    pub async fn start(
        conn: Connection,
        remote_forwards: RemoteForwards,
        spec: ForwardSpec,
        on_stop: StopNotifier,
    ) -> Result<Self, GatewayError> {
        let conn = Arc::new(conn);
        let cancel = CancellationToken::new();
        let mut remote = None;
        match spec.kind {
            ForwardKind::Local | ForwardKind::Dynamic => {
                let listener = TcpListener::bind((spec.bind_address.as_str(), spec.bind_port))
                    .await
                    .map_err(|e| GatewayError::Io(format!("cannot listen on {}:{}: {e}", spec.bind_address, spec.bind_port)))?;
                tokio::spawn(accept_loop(listener, Arc::clone(&conn), spec.clone(), cancel.clone()));
            }
            ForwardKind::Remote => {
                let (host, port) = target(&spec)?;
                let bound = conn
                    .target()
                    .tcpip_forward(spec.bind_address.as_str(), u32::from(spec.bind_port))
                    .await
                    .map_err(connection_error)?;
                let bound = if bound == 0 { u32::from(spec.bind_port) } else { bound };
                remote_forwards.add(bound, host, port);
                remote = Some((spec.bind_address.clone(), bound));
            }
        }
        tokio::spawn(watch(Arc::clone(&conn), cancel.clone(), on_stop));
        Ok(Self {
            conn,
            cancel,
            remote,
        })
    }
}

#[async_trait]
impl Tunnel for SshTunnel {
    async fn stop(&self) {
        self.cancel.cancel();
        if let Some((address, port)) = &self.remote {
            let _ = self.conn.target().cancel_tcpip_forward(address.as_str(), *port).await;
        }
        self.conn.disconnect().await;
    }
}

fn target(spec: &ForwardSpec) -> Result<(String, u16), GatewayError> {
    match (&spec.target_host, spec.target_port) {
        (Some(host), Some(port)) => Ok((host.clone(), port)),
        _ => Err(GatewayError::Connection("forward target is missing".into())),
    }
}

async fn watch(conn: Arc<Connection>, cancel: CancellationToken, on_stop: StopNotifier) {
    let on_stop = Mutex::new(Some(on_stop));
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(HEALTH_INTERVAL) => {
                if conn.is_closed() {
                    cancel.cancel();
                    if let Some(notify) = on_stop.lock().ok().and_then(|mut n| n.take()) {
                        notify(Some("connection lost".into()));
                    }
                    return;
                }
            }
        }
    }
}

async fn accept_loop(listener: TcpListener, conn: Arc<Connection>, spec: ForwardSpec, cancel: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(%err, "accept failed");
                continue;
            }
        };
        let conn = Arc::clone(&conn);
        let spec = spec.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let work = async {
                match spec.kind {
                    ForwardKind::Dynamic => serve_socks(&conn, stream, peer).await,
                    _ => serve_direct(&conn, stream, peer, &spec).await,
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                result = work => {
                    if let Err(err) = result {
                        tracing::debug!(%err, "forwarded connection ended");
                    }
                }
            }
        });
    }
}

async fn pipe(
    conn: &Connection,
    mut stream: TcpStream,
    peer: SocketAddr,
    host: &str,
    port: u16,
) -> Result<(), GatewayError> {
    let channel = conn
        .target()
        .channel_open_direct_tcpip(host, u32::from(port), peer.ip().to_string(), u32::from(peer.port()))
        .await
        .map_err(connection_error)?;
    let mut remote = channel.into_stream();
    tokio::io::copy_bidirectional(&mut stream, &mut remote)
        .await
        .map_err(|e| GatewayError::Io(e.to_string()))?;
    Ok(())
}

async fn serve_direct(conn: &Connection, stream: TcpStream, peer: SocketAddr, spec: &ForwardSpec) -> Result<(), GatewayError> {
    let (host, port) = target(spec)?;
    pipe(conn, stream, peer, &host, port).await
}

async fn serve_socks(conn: &Connection, stream: TcpStream, peer: SocketAddr) -> Result<(), GatewayError> {
    let socks = |e: fast_socks5::server::SocksServerError| GatewayError::Io(e.to_string());
    let (proto, command, address) = Socks5ServerProtocol::accept_no_auth(stream)
        .await
        .map_err(socks)?
        .read_command()
        .await
        .map_err(socks)?;
    if command != Socks5Command::TCPConnect {
        proto.reply_error(&ReplyError::CommandNotSupported).await.map_err(socks)?;
        return Ok(());
    }
    let (host, port) = match address {
        TargetAddr::Ip(addr) => (addr.ip().to_string(), addr.port()),
        TargetAddr::Domain(domain, port) => (domain, port),
    };
    let stream = proto
        .reply_success(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))
        .await
        .map_err(socks)?;
    pipe(conn, stream, peer, &host, port).await
}
