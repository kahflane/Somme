use std::sync::Arc;

use anyhow::Context;
use somme_application::ports::{CloudAccounts, ConnectContext, EventPublisher, SessionStore, SshGateway};
use somme_application::services::{
    ConnectionPlanner, ForwardService, GroupService, HostService, IdentityService, ImportService,
    KeyService, KnownHostService, KnownHostsVerifier, Links, SessionService, SftpService,
    SnippetService, SyncService, VaultService,
};
use somme_infrastructure::cloud::{DEFAULT_CLOUD_URL, HttpCloudClient, KeyringSessionStore, ManagedTokens};
use somme_infrastructure::crypto::XChaChaVaultCrypto;
use somme_infrastructure::db::{DataPaths, SqlRepository, TursoSync};
use somme_infrastructure::keystore::KeyringStore;
use somme_infrastructure::keytool::RusshKeyTool;
use somme_infrastructure::pty::NativePtyGateway;
use somme_infrastructure::ssh::RusshGateway;
use somme_infrastructure::ssh_config::OpenSshConfigSource;
use tauri::{AppHandle, Manager};

use crate::events::{TauriEvents, TauriPrompter};

pub struct AppState {
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
    pub prompter: Arc<TauriPrompter>,
}

impl AppState {
    pub async fn build(app: &AppHandle) -> anyhow::Result<Self> {
        let data_dir = match std::env::var_os("SOMME_DATA_DIR") {
            Some(dir) => dir.into(),
            None => app
                .path()
                .app_data_dir()
                .context("cannot resolve the app data directory")?,
        };
        let paths = DataPaths::new(data_dir).context("cannot create the app data directory")?;
        let cloud_url = std::env::var("SOMME_CLOUD_URL")
            .ok()
            .or_else(|| option_env!("SOMME_CLOUD_URL").map(ToOwned::to_owned))
            .unwrap_or_else(|| DEFAULT_CLOUD_URL.to_owned());
        let cloud: Arc<dyn CloudAccounts> = Arc::new(HttpCloudClient::new(&cloud_url)?);
        let cloud_sessions: Arc<dyn SessionStore> = Arc::new(KeyringSessionStore::default());
        let tokens = Arc::new(ManagedTokens::new(cloud.clone(), cloud_sessions.clone()));
        let sync_gateway = TursoSync::open(paths, tokens)
            .await
            .context("cannot open the database")?;
        let repo = Arc::new(SqlRepository::new(sync_gateway.store()));

        let events: Arc<dyn EventPublisher> = Arc::new(TauriEvents::new(app.clone()));
        let prompter = Arc::new(TauriPrompter::new(app.clone()));
        let ssh: Arc<dyn SshGateway> = Arc::new(RusshGateway);

        let vault = Arc::new(VaultService::new(
            repo.clone(),
            Arc::new(XChaChaVaultCrypto),
            Arc::new(KeyringStore::master_key()),
        ));

        let links = Arc::new(Links::new(repo.clone(), repo.clone(), repo.clone()));
        let ctx = ConnectContext {
            prompter: prompter.clone(),
            verifier: Arc::new(KnownHostsVerifier::new(repo.clone(), prompter.clone())),
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
            events.clone(),
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
            events.clone(),
        ));
        sync_gateway.start(events);

        Ok(Self {
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
            prompter,
        })
    }

    pub async fn shutdown(&self) {
        self.sessions.close_all().await;
        self.forwards.stop_all().await;
    }
}
