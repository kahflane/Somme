use std::sync::{Arc, Mutex};

use somme_application::ports::{
    CloudAccounts, CloudError, CloudSession, GatewayError, SessionStore,
};
use somme_domain::{Repository, Snippet, new_id, now};
use somme_infrastructure::cloud::{HttpCloudClient, ManagedTokens};
use somme_infrastructure::db::{Backend, SqlRepository, Store, TokenSource};

#[derive(Default)]
struct MemorySessions(Mutex<Option<CloudSession>>);

impl SessionStore for MemorySessions {
    fn load(&self) -> Option<CloudSession> {
        self.0.lock().unwrap().clone()
    }

    fn save(&self, session: &CloudSession) -> Result<(), GatewayError> {
        *self.0.lock().unwrap() = Some(session.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), GatewayError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

fn cloud_url() -> Option<String> {
    std::env::var("SOMME_CLOUD_URL").ok()
}

#[tokio::test]
#[ignore = "needs a running somme-cloud at SOMME_CLOUD_URL and a person to sign in with GitHub"]
async fn sign_in_then_two_devices_sync_through_managed_database() {
    let Some(url) = cloud_url() else { return };
    let cloud: Arc<dyn CloudAccounts> = Arc::new(HttpCloudClient::new(&url).unwrap());
    let session = cloud.sign_in_with_github().await.unwrap();
    assert!(!session.profile.login.is_empty());

    let credentials = cloud.credentials(&session).await.unwrap();
    assert!(credentials.url.starts_with("libsql://somme-"));

    let device = |session: CloudSession| {
        let sessions = Arc::new(MemorySessions::default());
        sessions.save(&session).unwrap();
        let tokens: Arc<dyn TokenSource> = Arc::new(ManagedTokens::new(cloud.clone(), sessions));
        tokens
    };
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();

    let a = Backend::open_synced(&dir_a.path().join("a.db"), &credentials.url, device(session.clone()))
        .await
        .unwrap();
    let store_a = Arc::new(Store::new(a));
    let repo_a = SqlRepository::new(Arc::clone(&store_a));
    let snippet = Snippet {
        id: new_id(),
        label: "managed".into(),
        script: "echo managed".into(),
        created_at: now(),
        updated_at: now(),
    };
    repo_a.save(&snippet).await.unwrap();
    store_a.backend().synced().unwrap().push().await.unwrap();

    // GitHub remembers the app, so the second sign-in finishes without a click.
    let second_login = cloud.sign_in_with_github().await.unwrap();
    let b = Backend::open_synced(&dir_b.path().join("b.db"), &credentials.url, device(second_login.clone()))
        .await
        .unwrap();
    b.synced().unwrap().pull().await.unwrap();
    let repo_b = SqlRepository::new(Arc::new(Store::new(b)));
    let seen: Option<Snippet> = repo_b.get(snippet.id).await.unwrap();
    assert_eq!(seen.map(|s| s.script), Some("echo managed".to_string()));

    cloud.logout(&second_login).await.unwrap();
    assert!(matches!(
        cloud.credentials(&second_login).await,
        Err(CloudError::SessionExpired)
    ));
    cloud.logout(&session).await.unwrap();
}
