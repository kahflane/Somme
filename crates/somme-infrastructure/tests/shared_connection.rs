use std::sync::Arc;

use somme_domain::{Repository, Snippet, new_id, now};
use somme_infrastructure::db::{Backend, SqlRepository, Store};

/// Every repository shares one connection, so reads and writes from many tasks overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_queries_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::open_local(&dir.path().join("test.db")).await.unwrap();
    let repo = SqlRepository::new(Arc::new(Store::new(backend)));

    let tasks: Vec<_> = (0..32)
        .map(|i| {
            let repo = repo.clone();
            tokio::spawn(async move {
                let snippet = Snippet {
                    id: new_id(),
                    label: format!("snippet {i}"),
                    script: "echo hi".into(),
                    created_at: now(),
                    updated_at: now(),
                };
                repo.save(&snippet).await?;
                Repository::<Snippet>::list(&repo).await.map(|_| ())
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }

    let saved: Vec<Snippet> = repo.list().await.unwrap();
    assert_eq!(saved.len(), 32);
}
