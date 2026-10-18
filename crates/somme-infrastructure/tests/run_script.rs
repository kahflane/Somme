//! `run_script` against a small in-process SSH server that answers with
//! canned output. Nothing runs on this computer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use somme_application::ports::{
    ConnectContext, ConnectPlan, Endpoint, HostKeyVerifier, PromptAnswer, PromptKind, Prompter, ServerKey, SshGateway,
};
use somme_infrastructure::ssh::RusshGateway;
use tokio::net::TcpListener;
use zeroize::Zeroizing;

#[derive(Clone)]
struct Canned;

impl russh::server::Handler for Canned {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == "pw" { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        match data {
            b"hello" => {
                session.data(channel, b"hi\n".to_vec())?;
                session.extended_data(channel, 1, b"careful\n".to_vec())?;
                session.exit_status_request(channel, 3)?;
            }
            b"flood" => {
                for _ in 0..100 {
                    session.data(channel, vec![b'x'; 1000])?;
                }
                session.data(channel, b"done".to_vec())?;
                session.exit_status_request(channel, 0)?;
            }
            // Never ends, like `sleep 1000`.
            b"hang" => return Ok(()),
            _ => session.exit_status_request(channel, 127)?,
        }
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

async fn serve() -> u16 {
    let config = Arc::new(russh::server::Config {
        keys: vec![russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap()],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let config = config.clone();
            tokio::spawn(async move {
                if let Ok(session) = russh::server::run_stream(config, stream, Canned).await {
                    let _ = session.await;
                }
            });
        }
    });
    port
}

struct Refuse;

#[async_trait]
impl Prompter for Refuse {
    async fn ask(&self, _title: &str, _kind: PromptKind) -> PromptAnswer {
        PromptAnswer::Cancel
    }
}

struct Trust;

#[async_trait]
impl HostKeyVerifier for Trust {
    async fn verify(&self, _title: &str, _host: &str, _port: u16, _key: &ServerKey) -> bool {
        true
    }
}

fn plan(port: u16) -> (ConnectPlan, ConnectContext) {
    let plan = ConnectPlan {
        hops: vec![Endpoint {
            title: "canned".into(),
            host: "127.0.0.1".into(),
            port,
            username: "agent".into(),
            password: Some(Zeroizing::new("pw".into())),
            keys: Vec::new(),
        }],
    };
    let ctx = ConnectContext {
        prompter: Arc::new(Refuse),
        verifier: Arc::new(Trust),
    };
    (plan, ctx)
}

#[tokio::test]
async fn script_output_and_exit_code_come_back() {
    let port = serve().await;
    let (plan, ctx) = plan(port);
    let out = RusshGateway.run_script(plan, ctx, "hello", Duration::from_secs(10), 1024).await.unwrap();
    assert_eq!(out.stdout, b"hi\n");
    assert_eq!(out.stderr, b"careful\n");
    assert_eq!(out.exit_status, Some(3));
    assert!(!out.timed_out);
}

#[tokio::test]
async fn long_output_is_cut_in_the_middle() {
    let port = serve().await;
    let (plan, ctx) = plan(port);
    let out = RusshGateway.run_script(plan, ctx, "flood", Duration::from_secs(10), 4096).await.unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("xxxx"));
    assert!(text.ends_with("xxdone"));
    assert!(text.contains("bytes cut"));
    assert!(text.len() < 4200);
    assert_eq!(out.exit_status, Some(0));
}

#[tokio::test]
async fn a_script_that_runs_too_long_is_stopped() {
    let port = serve().await;
    let (plan, ctx) = plan(port);
    let started = Instant::now();
    let out = RusshGateway.run_script(plan, ctx, "hang", Duration::from_millis(500), 1024).await.unwrap();
    assert!(out.timed_out);
    assert_eq!(out.exit_status, None);
    assert!(started.elapsed() < Duration::from_secs(5));
}
