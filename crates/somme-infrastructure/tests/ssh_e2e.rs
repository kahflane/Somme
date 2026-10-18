use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use somme_application::ports::{
    ConnectContext, ConnectPlan, Endpoint, ForwardSpec, GatewayError, HostKeyVerifier,
    KeyCredential, OutputSink, PromptAnswer, PromptKind, Prompter, ServerKey, SshGateway, TermSize,
};
use somme_domain::ForwardKind;
use somme_infrastructure::ssh::RusshGateway;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

const PORT: u16 = 2222;

fn fixtures() -> Option<PathBuf> {
    std::env::var("SOMME_TEST_SSHD_DIR").ok().map(PathBuf::from)
}

fn user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
}

fn key(dir: &std::path::Path, name: &str, passphrase: Option<&str>) -> KeyCredential {
    KeyCredential {
        label: name.into(),
        private_key: Zeroizing::new(std::fs::read_to_string(dir.join(name)).unwrap()),
        passphrase: passphrase.map(|p| Zeroizing::new(p.to_owned())),
    }
}

fn hop(keys: Vec<KeyCredential>) -> Endpoint {
    Endpoint {
        title: "local sshd".into(),
        host: "127.0.0.1".into(),
        port: PORT,
        username: user(),
        password: None,
        keys,
    }
}

struct ScriptedPrompter {
    passphrase: Option<String>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl Prompter for ScriptedPrompter {
    async fn ask(&self, _title: &str, kind: PromptKind) -> PromptAnswer {
        let label = match &kind {
            PromptKind::Password { .. } => "password",
            PromptKind::Passphrase { .. } => "passphrase",
            PromptKind::KeyboardInteractive { .. } => "keyboard",
            PromptKind::HostKey { .. } => "host_key",
        };
        self.asked.lock().unwrap().push(label.into());
        match (kind, &self.passphrase) {
            (PromptKind::Passphrase { .. }, Some(p)) => PromptAnswer::Answers(vec![p.clone()]),
            _ => PromptAnswer::Cancel,
        }
    }
}

struct FixedVerifier {
    accept: bool,
    seen: Mutex<Vec<ServerKey>>,
}

#[async_trait]
impl HostKeyVerifier for FixedVerifier {
    async fn verify(&self, _title: &str, _host: &str, _port: u16, key: &ServerKey) -> bool {
        self.seen.lock().unwrap().push(key.clone());
        self.accept
    }
}

fn ctx(accept: bool, passphrase: Option<&str>) -> (ConnectContext, Arc<ScriptedPrompter>, Arc<FixedVerifier>) {
    let prompter = Arc::new(ScriptedPrompter {
        passphrase: passphrase.map(Into::into),
        asked: Mutex::new(vec![]),
    });
    let verifier = Arc::new(FixedVerifier {
        accept,
        seen: Mutex::new(vec![]),
    });
    let ctx = ConnectContext {
        prompter: prompter.clone(),
        verifier: verifier.clone(),
    };
    (ctx, prompter, verifier)
}

struct CollectSink {
    tx: mpsc::UnboundedSender<Option<Vec<u8>>>,
}

impl OutputSink for CollectSink {
    fn data(&self, bytes: &[u8]) {
        let _ = self.tx.send(Some(bytes.to_vec()));
    }

    fn closed(&self, _reason: Option<String>) {
        let _ = self.tx.send(None);
    }
}

async fn run_shell(plan: ConnectPlan, ctx: ConnectContext) -> String {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let terminal = RusshGateway
        .shell(plan, ctx, TermSize { cols: 80, rows: 24 }, None, Arc::new(CollectSink { tx }))
        .await
        .unwrap();
    terminal.write(b"echo hello-$((40+2)); exit\n").await.unwrap();
    let mut out = Vec::new();
    let collected = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Some(chunk)) = rx.recv().await {
            out.extend(chunk);
        }
    })
    .await;
    assert!(collected.is_ok(), "shell did not close in time");
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn shell_with_plain_key() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, verifier) = ctx(true, None);
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let out = run_shell(plan, ctx).await;
    assert!(out.contains("hello-42"), "output was: {out}");
    let seen = verifier.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].key_type, "ssh-ed25519");
    assert!(seen[0].fingerprint.starts_with("SHA256:"));
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn encrypted_key_asks_for_passphrase() {
    let Some(dir) = fixtures() else { return };
    let (ctx, prompter, _) = ctx(true, Some("pass phrase"));
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key_enc", None)])],
    };
    let out = run_shell(plan, ctx).await;
    assert!(out.contains("hello-42"));
    assert_eq!(*prompter.asked.lock().unwrap(), vec!["passphrase".to_string()]);
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn rejected_host_key_stops_connection() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, _) = ctx(false, None);
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let result = RusshGateway.sftp(plan, ctx).await;
    assert!(matches!(result, Err(GatewayError::HostKeyRejected)));
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn wrong_key_fails_auth_without_password_prompt_loop() {
    let Some(_) = fixtures() else { return };
    let (ctx, _, _) = ctx(true, None);
    let plan = ConnectPlan { hops: vec![hop(vec![])] };
    let result = RusshGateway.sftp(plan, ctx).await;
    assert!(matches!(result, Err(GatewayError::AuthFailed(_))), "{:?}", result.err());
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn jump_host_chain() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, verifier) = ctx(true, None);
    let plan = ConnectPlan {
        hops: vec![
            hop(vec![key(&dir, "client_key", None)]),
            hop(vec![key(&dir, "client_key", None)]),
        ],
    };
    let out = run_shell(plan, ctx).await;
    assert!(out.contains("hello-42"));
    assert_eq!(verifier.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn sftp_round_trip() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, _) = ctx(true, None);
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let fs = RusshGateway.sftp(plan, ctx).await.unwrap();
    assert!(fs.home().await.unwrap().starts_with('/'));
    let work = dir.join("sftp-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let remote_dir = format!("{}/remote", work.display());
    fs.mkdir(&remote_dir).await.unwrap();
    let local = work.join("up.txt");
    std::fs::write(&local, b"somme sftp payload").unwrap();
    let remote_file = format!("{remote_dir}/up.txt");
    fs.upload(&local, &remote_file).await.unwrap();
    let listed = fs.list(&remote_dir).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "up.txt");
    assert_eq!(listed[0].size, 18);
    assert!(!listed[0].is_dir);
    let renamed = format!("{remote_dir}/renamed.txt");
    fs.rename(&remote_file, &renamed).await.unwrap();
    let down = work.join("down.txt");
    fs.download(&renamed, &down).await.unwrap();
    assert_eq!(std::fs::read(&down).unwrap(), b"somme sftp payload");
    fs.remove(&remote_dir, true).await.unwrap();
    assert!(!PathBuf::from(&remote_dir).exists());
    fs.close().await;
}

/// The shell loads Somme's hooks on the server, and the server's files open
/// over the same connection.
#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn shell_with_hooks_marks_prompts_and_shares_files() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, _) = ctx(true, None);
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let terminal = RusshGateway
        .shell(plan, ctx, TermSize { cols: 80, rows: 24 }, Some("t0k3n"), Arc::new(CollectSink { tx }))
        .await
        .unwrap();
    assert!(terminal.has_hooks());

    let work = dir.join("hooks-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let files = terminal.files().await.expect("files beside the shell");
    let note = format!("{}/note.txt", work.display());
    files.write(&note, b"a longer first version").await.unwrap();
    files.write(&note, b"from somme").await.unwrap();
    assert_eq!(files.read(&note, 1024).await.unwrap(), b"from somme");
    assert!(files.read(&note, 3).await.is_err(), "over the limit");
    assert!(!files.stat(&note).await.unwrap().is_dir);
    assert!(files.stat(&work.display().to_string()).await.unwrap().is_dir);

    terminal
        .write(format!("cd '{}'; cat note.txt; exit\n", work.display()).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    let collected = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Some(chunk)) = rx.recv().await {
            out.extend(chunk);
        }
    })
    .await;
    assert!(collected.is_ok(), "shell did not close in time");
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("\x1b]6973;t0k3n;H;"), "{out:?}");
    assert!(out.contains("\x1b]6973;t0k3n;A;0;"), "{out:?}");
    assert!(out.contains("\x1b]6973;t0k3n;C;cd "), "{out:?}");
    assert!(out.contains("from somme"), "{out:?}");
    terminal.close().await;
    std::fs::remove_dir_all(&work).unwrap();
}

async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn read_banner(mut stream: TcpStream) -> String {
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn local_and_dynamic_forwarding() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, _) = ctx(true, None);
    let local_port = free_port().await;
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let tunnel = RusshGateway
        .forward(
            plan,
            ctx.clone(),
            ForwardSpec {
                kind: ForwardKind::Local,
                bind_address: "127.0.0.1".into(),
                bind_port: local_port,
                target_host: Some("127.0.0.1".into()),
                target_port: Some(PORT),
            },
            Box::new(|_| {}),
        )
        .await
        .unwrap();
    let banner = read_banner(TcpStream::connect(("127.0.0.1", local_port)).await.unwrap()).await;
    assert!(banner.starts_with("SSH-2.0"), "banner: {banner}");
    tunnel.stop().await;

    let socks_port = free_port().await;
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let socks = RusshGateway
        .forward(
            plan,
            ctx,
            ForwardSpec {
                kind: ForwardKind::Dynamic,
                bind_address: "127.0.0.1".into(),
                bind_port: socks_port,
                target_host: None,
                target_port: None,
            },
            Box::new(|_| {}),
        )
        .await
        .unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", socks_port)).await.unwrap();
    stream.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let port = PORT.to_be_bytes();
    stream
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, port[0], port[1]])
        .await
        .unwrap();
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0, "socks reply: {reply:?}");
    let banner = read_banner(stream).await;
    assert!(banner.starts_with("SSH-2.0"), "banner: {banner}");
    socks.stop().await;
}

#[tokio::test]
#[ignore = "needs SOMME_TEST_SSHD_DIR with a running sshd on 127.0.0.1:2222"]
async fn remote_forwarding() {
    let Some(dir) = fixtures() else { return };
    let (ctx, _, _) = ctx(true, None);
    let remote_port = free_port().await;
    let plan = ConnectPlan {
        hops: vec![hop(vec![key(&dir, "client_key", None)])],
    };
    let tunnel = RusshGateway
        .forward(
            plan,
            ctx,
            ForwardSpec {
                kind: ForwardKind::Remote,
                bind_address: "127.0.0.1".into(),
                bind_port: remote_port,
                target_host: Some("127.0.0.1".into()),
                target_port: Some(PORT),
            },
            Box::new(|_| {}),
        )
        .await
        .unwrap();
    let banner = read_banner(TcpStream::connect(("127.0.0.1", remote_port)).await.unwrap()).await;
    assert!(banner.starts_with("SSH-2.0"), "banner: {banner}");
    tunnel.stop().await;
}
