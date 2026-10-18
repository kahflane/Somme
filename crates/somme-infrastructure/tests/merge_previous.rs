use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use somme_application::ports::{
    CloudAccounts, CloudError, CloudSession, CryptoError, EventPublisher, GatewayError, KeyStore, MasterKey,
    SessionStore, SyncCredentials, SyncState,
};
use somme_application::services::{SyncService, VaultService};
use somme_domain::{Host, Id, Repository, Snippet, SshKey, new_id, now};
use somme_infrastructure::crypto::XChaChaVaultCrypto;
use somme_infrastructure::db::{Backend, DataPaths, SqlRepository, StaticToken, Store, SyncConfig, TursoSync};

#[derive(Default)]
struct MemoryKeys(Mutex<Option<MasterKey>>);

impl KeyStore for MemoryKeys {
    fn load(&self) -> Option<MasterKey> {
        self.0.lock().unwrap().clone()
    }

    fn store(&self, key: &MasterKey) -> Result<(), CryptoError> {
        *self.0.lock().unwrap() = Some(key.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), CryptoError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

struct NoCloud;

#[async_trait]
impl CloudAccounts for NoCloud {
    async fn sign_in_with_github(&self) -> Result<CloudSession, CloudError> {
        unimplemented!()
    }

    async fn logout(&self, _: &CloudSession) -> Result<(), CloudError> {
        unimplemented!()
    }

    async fn credentials(&self, _: &CloudSession) -> Result<SyncCredentials, CloudError> {
        unimplemented!()
    }

    async fn avatar(&self, _: &str) -> Result<Vec<u8>, CloudError> {
        unimplemented!()
    }
}

struct NoSessions;

impl SessionStore for NoSessions {
    fn load(&self) -> Option<CloudSession> {
        None
    }

    fn save(&self, _: &CloudSession) -> Result<(), GatewayError> {
        Ok(())
    }

    fn clear(&self) -> Result<(), GatewayError> {
        Ok(())
    }
}

struct NoEvents;

impl EventPublisher for NoEvents {
    fn data_changed(&self) {}
    fn sync_status(&self, _: SyncState) {}
    fn forward_stopped(&self, _: Id, _: Option<String>) {}
}

fn vault(repo: Arc<SqlRepository>) -> VaultService {
    VaultService::new(repo, Arc::new(XChaChaVaultCrypto), Arc::new(MemoryKeys::default()))
}

/// Fills a device's own vault with a host password, a key and a snippet.
async fn old_device(path: &Path, password: &str) -> (Host, SshKey, Snippet) {
    let repo = Arc::new(SqlRepository::new(Arc::new(Store::new(Backend::open_local(path).await.unwrap()))));
    let vault = vault(repo.clone());
    vault.setup(password, false).await.unwrap();
    let host = Host {
        id: new_id(),
        label: "db".into(),
        address: "10.0.0.5".into(),
        port: 22,
        group_id: None,
        identity_id: None,
        username: Some("root".into()),
        password: Some(vault.seal("host secret").unwrap()),
        key_id: None,
        jump_host_id: None,
        startup_snippet_id: None,
        tags: vec![],
        color: None,
        created_at: now(),
        updated_at: now(),
    };
    let key = SshKey {
        id: new_id(),
        label: "laptop".into(),
        algorithm: "ssh-ed25519".into(),
        public_key: "ssh-ed25519 AAAA".into(),
        fingerprint: "SHA256:x".into(),
        private_key: vault.seal("PRIVATE KEY").unwrap(),
        passphrase: Some(vault.seal("key phrase").unwrap()),
        created_at: now(),
        updated_at: now(),
    };
    let snippet = Snippet {
        id: new_id(),
        label: "uptime".into(),
        script: "uptime".into(),
        created_at: now(),
        updated_at: now(),
    };
    repo.save(&host).await.unwrap();
    repo.save(&key).await.unwrap();
    repo.save(&snippet).await.unwrap();
    (host, key, snippet)
}

/// A device that joined an account whose vault uses `account_password`.
async fn joined(dir: &Path, old_db: &Path, account_password: &str) -> (Arc<SqlRepository>, Arc<VaultService>, SyncService) {
    let paths = DataPaths::new(dir.join("new")).unwrap();
    SyncConfig {
        url: None,
        enabled: false,
        previous: Some(old_db.to_owned()),
    }
    .save(&paths)
    .unwrap();
    let gateway = TursoSync::open(paths, Arc::new(StaticToken(String::new()))).await.unwrap();
    let repo = Arc::new(SqlRepository::new(gateway.store()));
    let vault = Arc::new(vault(repo.clone()));
    vault.setup(account_password, false).await.unwrap();
    let sync = SyncService::new(
        gateway,
        Arc::new(NoCloud),
        Arc::new(NoSessions),
        vault.clone(),
        Arc::new(NoEvents),
        repo.clone(),
    );
    (repo, vault, sync)
}

#[tokio::test]
async fn same_master_password_moves_secrets_into_the_account_vault() {
    let dir = tempfile::tempdir().unwrap();
    let old_db = dir.path().join("old.db");
    let (host, key, snippet) = old_device(&old_db, "same master password").await;
    let (repo, vault, sync) = joined(dir.path(), &old_db, "same master password").await;

    let report = sync.merge_previous("same master password").await.unwrap();
    assert_eq!((report.added, report.secrets_lost), (3, 0));

    let moved: Host = repo.get(host.id).await.unwrap().unwrap();
    assert_eq!(vault.open(moved.password.as_ref().unwrap()).unwrap().as_str(), "host secret");
    let moved: SshKey = repo.get(key.id).await.unwrap().unwrap();
    assert_eq!(vault.open(&moved.private_key).unwrap().as_str(), "PRIVATE KEY");
    assert_eq!(vault.open(moved.passphrase.as_ref().unwrap()).unwrap().as_str(), "key phrase");
    assert!(Repository::<Snippet>::get(&*repo, snippet.id).await.unwrap().is_some());

    let again = sync.merge_previous("same master password").await.unwrap();
    assert_eq!(again.added, 0);
}

#[tokio::test]
async fn other_master_password_keeps_secrets_out() {
    let dir = tempfile::tempdir().unwrap();
    let old_db = dir.path().join("old.db");
    let (host, key, snippet) = old_device(&old_db, "this device password").await;
    let (repo, _vault, sync) = joined(dir.path(), &old_db, "the account password").await;

    let report = sync.merge_previous("the account password").await.unwrap();
    assert_eq!((report.added, report.secrets_lost), (2, 2));
    let moved: Host = repo.get(host.id).await.unwrap().unwrap();
    assert!(moved.password.is_none());
    assert!(Repository::<SshKey>::get(&*repo, key.id).await.unwrap().is_none());
    assert!(Repository::<Snippet>::get(&*repo, snippet.id).await.unwrap().is_some());
}
