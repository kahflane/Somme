use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use somme_application::ports::{
    CloudAccounts, CloudSession, ConnectContext, CryptoError, EventPublisher, GatewayError, KeyStore, MasterKey,
    PromptAnswer, PromptKind, Prompter, SessionStore, SshGateway, SyncState,
};
use somme_application::services::{
    ConnectionPlanner, ForwardService, GroupService, HostService, IdentityService, ImportService, KeyService,
    KnownHostService, KnownHostsVerifier, Links, SessionService, SftpService, SnippetService, SyncService,
    VaultService,
};
use somme_domain::Id;
use somme_infrastructure::cloud::{DEFAULT_CLOUD_URL, HttpCloudClient, KeyringSessionStore, ManagedTokens};
use somme_infrastructure::crypto::XChaChaVaultCrypto;
use somme_infrastructure::db::{DataPaths, SqlRepository, TursoSync};
use somme_infrastructure::keystore::KeyringStore;
use somme_infrastructure::keytool::RusshKeyTool;
use somme_infrastructure::pty::NativePtyGateway;
use somme_infrastructure::ssh::RusshGateway;
use somme_infrastructure::ssh_config::OpenSshConfigSource;

/// Same folder the Tauri build used, so an existing vault keeps working.
const APP_IDENTIFIER: &str = "app.somme.desktop";
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// Something the backend wants the UI to know about.
pub enum BackendEvent {
    DataChanged,
    SyncStatus(SyncState),
    ForwardStopped { rule_id: Id, reason: Option<String> },
    Prompt(PromptRequest),
}

/// A question the backend is waiting on, like a password or a new host key.
pub struct PromptRequest {
    pub title: String,
    pub kind: PromptKind,
    pub answer: oneshot::Sender<PromptAnswer>,
}

struct ChannelEvents(mpsc::UnboundedSender<BackendEvent>);

impl EventPublisher for ChannelEvents {
    fn data_changed(&self) {
        let _ = self.0.unbounded_send(BackendEvent::DataChanged);
    }

    fn sync_status(&self, state: SyncState) {
        let _ = self.0.unbounded_send(BackendEvent::SyncStatus(state));
    }

    fn forward_stopped(&self, rule_id: Id, reason: Option<String>) {
        let _ = self.0.unbounded_send(BackendEvent::ForwardStopped { rule_id, reason });
    }
}

struct ChannelPrompter(mpsc::UnboundedSender<BackendEvent>);

#[async_trait]
impl Prompter for ChannelPrompter {
    async fn ask(&self, title: &str, kind: PromptKind) -> PromptAnswer {
        let (answer, receiver) = oneshot::channel();
        let request = PromptRequest {
            title: title.to_owned(),
            kind,
            answer,
        };
        if self.0.unbounded_send(BackendEvent::Prompt(request)).is_err() {
            return PromptAnswer::Cancel;
        }
        match tokio::time::timeout(PROMPT_TIMEOUT, receiver).await {
            Ok(Ok(answer)) => answer,
            _ => PromptAnswer::Cancel,
        }
    }
}

/// Keeps secrets in memory only. Used when `SOMME_DATA_DIR` points at a test
/// folder, so test runs never read or replace the real keychain entries.
#[derive(Default)]
struct MemoryKeyStore(parking_lot::Mutex<Option<[u8; 32]>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self) -> Option<MasterKey> {
        (*self.0.lock()).map(MasterKey::from_bytes)
    }

    fn store(&self, key: &MasterKey) -> Result<(), CryptoError> {
        *self.0.lock() = Some(*key.as_bytes());
        Ok(())
    }

    fn clear(&self) -> Result<(), CryptoError> {
        *self.0.lock() = None;
        Ok(())
    }
}

#[derive(Default)]
struct MemorySessionStore(parking_lot::Mutex<Option<(String, String)>>);

impl SessionStore for MemorySessionStore {
    fn load(&self) -> Option<CloudSession> {
        let (token, email) = self.0.lock().clone()?;
        Some(CloudSession {
            token: token.into(),
            email,
        })
    }

    fn save(&self, session: &CloudSession) -> Result<(), GatewayError> {
        *self.0.lock() = Some((session.token.to_string(), session.email.clone()));
        Ok(())
    }

    fn clear(&self) -> Result<(), GatewayError> {
        *self.0.lock() = None;
        Ok(())
    }
}

fn is_test_data_dir() -> bool {
    std::env::var_os("SOMME_DATA_DIR").is_some()
}

pub struct Services {
    pub data_dir: PathBuf,
    pub vault: Arc<VaultService>,
    pub groups: Arc<GroupService>,
    pub hosts: Arc<HostService>,
    pub identities: Arc<IdentityService>,
    pub keys: Arc<KeyService>,
    pub snippets: Arc<SnippetService>,
    pub forwards: Arc<ForwardService>,
    pub known_hosts: Arc<KnownHostService>,
    pub sessions: Arc<SessionService>,
    pub sftp: Arc<SftpService>,
    pub sync: Arc<SyncService>,
    pub import: Arc<ImportService>,
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("SOMME_DATA_DIR") {
        return Ok(dir.into());
    }
    let base = dirs::data_dir().context("cannot find the user data folder")?;
    Ok(base.join(APP_IDENTIFIER))
}

impl Services {
    pub async fn build(events: mpsc::UnboundedSender<BackendEvent>) -> anyhow::Result<Self> {
        let data_dir = data_dir()?;
        let paths = DataPaths::new(&data_dir).context("cannot create the app data folder")?;
        let cloud_url = std::env::var("SOMME_CLOUD_URL")
            .ok()
            .or_else(|| option_env!("SOMME_CLOUD_URL").map(ToOwned::to_owned))
            .unwrap_or_else(|| DEFAULT_CLOUD_URL.to_owned());
        let cloud: Arc<dyn CloudAccounts> = Arc::new(HttpCloudClient::new(&cloud_url)?);
        let test_mode = is_test_data_dir();
        let cloud_sessions: Arc<dyn SessionStore> = if test_mode {
            Arc::new(MemorySessionStore::default())
        } else {
            Arc::new(KeyringSessionStore::default())
        };
        let master_key: Arc<dyn KeyStore> = if test_mode {
            Arc::new(MemoryKeyStore::default())
        } else {
            Arc::new(KeyringStore::master_key())
        };
        let tokens = Arc::new(ManagedTokens::new(cloud.clone(), cloud_sessions.clone()));
        let sync_gateway = TursoSync::open(paths, tokens)
            .await
            .context("cannot open the database")?;
        let repo = Arc::new(SqlRepository::new(sync_gateway.store()));

        let publisher: Arc<dyn EventPublisher> = Arc::new(ChannelEvents(events.clone()));
        let prompter: Arc<dyn Prompter> = Arc::new(ChannelPrompter(events));
        let ssh: Arc<dyn SshGateway> = Arc::new(RusshGateway);

        let vault = Arc::new(VaultService::new(
            repo.clone(),
            Arc::new(XChaChaVaultCrypto),
            master_key,
        ));
        let links = Arc::new(Links::new(repo.clone(), repo.clone(), repo.clone()));
        let ctx = ConnectContext {
            prompter: prompter.clone(),
            verifier: Arc::new(KnownHostsVerifier::new(repo.clone(), prompter)),
        };
        let planner = Arc::new(ConnectionPlanner::new(
            repo.clone(),
            repo.clone(),
            repo.clone(),
            vault.clone(),
        ));
        let forwards = Arc::new(ForwardService::new(
            repo.clone(),
            planner.clone(),
            ssh.clone(),
            ctx.clone(),
            publisher.clone(),
        ));
        let hosts = Arc::new(HostService::new(
            repo.clone(),
            vault.clone(),
            links.clone(),
            forwards.clone(),
        ));
        let keys = Arc::new(KeyService::new(
            repo.clone(),
            Arc::new(RusshKeyTool),
            vault.clone(),
            links.clone(),
        ));
        let snippets = Arc::new(SnippetService::new(repo.clone(), links.clone()));
        let sessions = Arc::new(SessionService::new(
            planner.clone(),
            snippets.clone(),
            ssh.clone(),
            Arc::new(NativePtyGateway),
            ctx.clone(),
        ));
        let import = Arc::new(ImportService::new(
            Arc::new(OpenSshConfigSource::user_default()),
            hosts.clone(),
            keys.clone(),
        ));
        let sync = Arc::new(SyncService::new(
            sync_gateway.clone(),
            cloud,
            cloud_sessions,
            vault.clone(),
            publisher.clone(),
        ));
        sync_gateway.start(publisher);

        Ok(Self {
            data_dir,
            groups: Arc::new(GroupService::new(repo.clone(), links.clone())),
            identities: Arc::new(IdentityService::new(repo.clone(), vault.clone(), links)),
            known_hosts: Arc::new(KnownHostService::new(repo)),
            sftp: Arc::new(SftpService::new(planner, ssh, ctx)),
            vault,
            hosts,
            keys,
            snippets,
            forwards,
            sessions,
            sync,
            import,
        })
    }

    pub async fn shutdown(&self) {
        self.sessions.close_all().await;
        self.forwards.stop_all().await;
    }
}
