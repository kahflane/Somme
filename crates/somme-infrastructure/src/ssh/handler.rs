use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dashmap::DashMap;
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{Channel, client};
use somme_application::ports::{HostKeyVerifier, ServerKey};
use tokio::net::TcpStream;

#[derive(Clone, Default)]
pub struct RemoteForwards(Arc<DashMap<u32, (String, u16)>>);

impl RemoteForwards {
    pub fn add(&self, bound_port: u32, target_host: String, target_port: u16) {
        self.0.insert(bound_port, (target_host, target_port));
    }

    fn target(&self, port: u32) -> Option<(String, u16)> {
        self.0.get(&port).map(|t| t.value().clone())
    }
}

pub struct ClientHandler {
    pub title: String,
    pub host: String,
    pub port: u16,
    pub verifier: Arc<dyn HostKeyVerifier>,
    pub rejected: Arc<AtomicBool>,
    pub remote_forwards: RemoteForwards,
}

fn describe(key: &PublicKeyOrCertificate) -> ServerKey {
    let public = key.public_key();
    ServerKey {
        key_type: public.algorithm().as_str().to_owned(),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        public_key: public.to_openssh().unwrap_or_default(),
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let accepted = self
            .verifier
            .verify(&self.title, &self.host, self.port, &describe(key))
            .await;
        if !accepted {
            self.rejected.store(true, Ordering::SeqCst);
        }
        Ok(accepted)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some((host, port)) = self.remote_forwards.target(connected_port) else {
            tracing::warn!(connected_address, connected_port, "unexpected forwarded connection");
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            match TcpStream::connect((host.as_str(), port)).await {
                Ok(mut local) => {
                    let mut remote = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
                }
                Err(err) => {
                    tracing::warn!(%err, host, port, "remote forward target unreachable");
                    let _ = channel.close().await;
                }
            }
        });
        Ok(())
    }
}
