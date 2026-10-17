use std::sync::Arc;

use async_trait::async_trait;
use somme_domain::{
    Id, KnownHost, KnownHostRepository, Repository, RepositoryError, Sealed, VaultMeta,
    VaultRepository,
};
use turso::Value;

use crate::db::Store;
use crate::db::records::{RowReader, SqlEntity};

const VAULT_ROW_ID: &str = "vault";

fn storage(err: turso::Error) -> RepositoryError {
    RepositoryError::Storage(err.to_string())
}

#[derive(Clone)]
pub struct SqlRepository {
    store: Arc<Store>,
}

impl SqlRepository {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    async fn query<T: SqlEntity>(
        &self,
        clause: &str,
        params: Vec<Value>,
    ) -> Result<Vec<T>, RepositoryError> {
        let sql = format!("SELECT {} FROM {} {clause}", T::COLUMNS.join(", "), T::TABLE);
        let mut rows = self.store.conn().query(sql, params).await.map_err(storage)?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().await.map_err(storage)? {
            items.push(T::from_row(&RowReader::new(T::TABLE, &row))?);
        }
        Ok(items)
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<(), RepositoryError> {
        self.store.conn().execute(sql, params).await.map_err(storage)?;
        self.store.mark_changed();
        Ok(())
    }
}

fn upsert_sql<T: SqlEntity>() -> String {
    let columns = T::COLUMNS.join(", ");
    let placeholders = vec!["?"; T::COLUMNS.len()].join(", ");
    let updates = T::COLUMNS
        .iter()
        .filter(|c| **c != "id")
        .map(|c| format!("{c} = excluded.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders}) ON CONFLICT(id) DO UPDATE SET {updates}",
        T::TABLE
    )
}

#[async_trait]
impl<T: SqlEntity> Repository<T> for SqlRepository {
    async fn list(&self) -> Result<Vec<T>, RepositoryError> {
        self.query("", vec![]).await
    }

    async fn get(&self, id: Id) -> Result<Option<T>, RepositoryError> {
        Ok(self
            .query("WHERE id = ?", vec![Value::Text(id.to_string())])
            .await?
            .into_iter()
            .next())
    }

    async fn save(&self, item: &T) -> Result<(), RepositoryError> {
        self.execute(&upsert_sql::<T>(), item.values()).await
    }

    async fn delete(&self, id: Id) -> Result<(), RepositoryError> {
        let sql = format!("DELETE FROM {} WHERE id = ?", T::TABLE);
        self.execute(&sql, vec![Value::Text(id.to_string())]).await
    }
}

#[async_trait]
impl KnownHostRepository for SqlRepository {
    async fn find(&self, host: &str, port: u16) -> Result<Option<KnownHost>, RepositoryError> {
        Ok(self
            .query::<KnownHost>(
                "WHERE host = ? AND port = ?",
                vec![Value::Text(host.to_owned()), Value::Integer(port.into())],
            )
            .await?
            .into_iter()
            .next())
    }
}

#[async_trait]
impl VaultRepository for SqlRepository {
    async fn load(&self) -> Result<Option<VaultMeta>, RepositoryError> {
        let mut rows = self
            .store
            .conn()
            .query(
                "SELECT salt, check_blob, created_at FROM vault_meta WHERE id = ?",
                vec![Value::Text(VAULT_ROW_ID.into())],
            )
            .await
            .map_err(storage)?;
        let Some(row) = rows.next().await.map_err(storage)? else {
            return Ok(None);
        };
        let r = RowReader::new("vault_meta", &row);
        Ok(Some(VaultMeta {
            salt: r.text(0)?,
            check: Sealed::new(r.text(1)?),
            created_at: r.int(2)?,
        }))
    }

    async fn store(&self, meta: &VaultMeta) -> Result<(), RepositoryError> {
        self.execute(
            "INSERT INTO vault_meta (id, salt, check_blob, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET salt = excluded.salt, check_blob = excluded.check_blob, \
             created_at = excluded.created_at",
            vec![
                Value::Text(VAULT_ROW_ID.into()),
                Value::Text(meta.salt.clone()),
                Value::Text(meta.check.as_str().to_owned()),
                Value::Integer(meta.created_at),
            ],
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::Backend;
    use somme_domain::{Host, Snippet, new_id, now};

    async fn repo() -> (tempfile::TempDir, SqlRepository) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Backend::open_local(&dir.path().join("t.db")).await.unwrap();
        (dir, SqlRepository::new(Arc::new(Store::new(backend))))
    }

    #[tokio::test]
    async fn upsert_get_list_delete() {
        let (_dir, repo) = repo().await;
        let mut snippet = Snippet {
            id: new_id(),
            label: "uptime".into(),
            script: "uptime".into(),
            created_at: now(),
            updated_at: now(),
        };
        Repository::<Snippet>::save(&repo, &snippet).await.unwrap();
        snippet.script = "uptime -p".into();
        Repository::<Snippet>::save(&repo, &snippet).await.unwrap();
        let all: Vec<Snippet> = repo.list().await.unwrap();
        assert_eq!(all, vec![snippet.clone()]);
        let one: Option<Snippet> = repo.get(snippet.id).await.unwrap();
        assert_eq!(one.unwrap().script, "uptime -p");
        Repository::<Snippet>::delete(&repo, snippet.id).await.unwrap();
        let none: Option<Snippet> = repo.get(snippet.id).await.unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn host_round_trip_with_nulls_and_tags() {
        let (_dir, repo) = repo().await;
        let host = Host {
            id: new_id(),
            label: "db".into(),
            address: "10.0.0.5".into(),
            port: 2222,
            group_id: None,
            identity_id: Some(new_id()),
            username: Some("root".into()),
            password: Some(Sealed::new("v1:abc")),
            key_id: None,
            jump_host_id: None,
            startup_snippet_id: None,
            tags: vec!["prod".into(), "eu".into()],
            color: None,
            created_at: 1,
            updated_at: 2,
        };
        Repository::<Host>::save(&repo, &host).await.unwrap();
        let loaded: Option<Host> = repo.get(host.id).await.unwrap();
        assert_eq!(loaded.unwrap(), host);
    }

    #[tokio::test]
    async fn vault_meta_and_known_host_lookup() {
        let (_dir, repo) = repo().await;
        assert!(VaultRepository::load(&repo).await.unwrap().is_none());
        let meta = VaultMeta {
            salt: "c2FsdA==".into(),
            check: Sealed::new("v1:x"),
            created_at: 5,
        };
        VaultRepository::store(&repo, &meta).await.unwrap();
        assert_eq!(VaultRepository::load(&repo).await.unwrap(), Some(meta));
        let known = KnownHost {
            id: new_id(),
            host: "example.com".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
            public_key: "AAAA".into(),
            created_at: 1,
        };
        Repository::<KnownHost>::save(&repo, &known).await.unwrap();
        assert_eq!(repo.find("example.com", 22).await.unwrap(), Some(known));
        assert_eq!(repo.find("example.com", 2222).await.unwrap(), None);
    }
}
