use std::sync::Arc;

use somme_domain::{Repository, Snippet, new_id, now};
use somme_infrastructure::db::{Backend, SqlRepository, StaticToken, Store, TokenSource};

fn remote() -> Option<(String, String)> {
    Some((
        std::env::var("TURSO_DATABASE_URL").ok()?,
        std::env::var("TURSO_AUTH_TOKEN").ok()?,
    ))
}

#[tokio::test]
#[ignore = "needs TURSO_DATABASE_URL and TURSO_AUTH_TOKEN"]
async fn two_devices_see_each_other() {
    let Some((url, token)) = remote() else {
        return;
    };
    let slot: Arc<dyn TokenSource> = Arc::new(StaticToken(token));
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();

    let a = Backend::open_synced(&dir_a.path().join("a.db"), &url, Arc::clone(&slot))
        .await
        .unwrap();
    a.synced().unwrap().push().await.unwrap();
    let store_a = Arc::new(Store::new(a));
    let repo_a = SqlRepository::new(Arc::clone(&store_a));
    let snippet = Snippet {
        id: new_id(),
        label: "sync-test".into(),
        script: "echo hello".into(),
        created_at: now(),
        updated_at: now(),
    };
    repo_a.save(&snippet).await.unwrap();
    store_a.backend().synced().unwrap().push().await.unwrap();

    let b = Backend::open_synced(&dir_b.path().join("b.db"), &url, Arc::clone(&slot))
        .await
        .unwrap();
    b.synced().unwrap().pull().await.unwrap();
    let store_b = Arc::new(Store::new(b));
    let repo_b = SqlRepository::new(Arc::clone(&store_b));
    let seen: Option<Snippet> = repo_b.get(snippet.id).await.unwrap();
    assert_eq!(seen.as_ref().map(|s| s.script.as_str()), Some("echo hello"));

    Repository::<Snippet>::delete(&repo_b, snippet.id).await.unwrap();
    store_b.backend().synced().unwrap().push().await.unwrap();
    store_a.backend().synced().unwrap().pull().await.unwrap();
    let gone: Option<Snippet> = repo_a.get(snippet.id).await.unwrap();
    assert!(gone.is_none());
}
