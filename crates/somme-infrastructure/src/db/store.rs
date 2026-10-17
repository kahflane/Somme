use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::sync::{Mutex, MutexGuard, Notify};
use turso::{Connection, Value};

use crate::db::migrations::{DATA_TABLES, migrate};

const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Turso(#[from] turso::Error),
    #[error("timed out while opening the database")]
    Timeout,
    #[error("invalid path: {0}")]
    Path(String),
}

#[async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self) -> Option<String>;
    fn available(&self) -> bool;
}

pub struct StaticToken(pub String);

#[async_trait]
impl TokenSource for StaticToken {
    async fn token(&self) -> Option<String> {
        Some(self.0.clone())
    }

    fn available(&self) -> bool {
        true
    }
}

pub struct Backend {
    path: PathBuf,
    conn: Connection,
    synced: Option<turso::sync::Database>,
    _local: Option<turso::Database>,
}

impl Backend {
    pub async fn open_local(path: &Path) -> Result<Self, StoreError> {
        let file = path_str(path)?;
        let db = turso::Builder::new_local(&file).build().await?;
        let conn = db.connect()?;
        migrate(&conn).await?;
        Ok(Self {
            path: path.to_owned(),
            conn,
            synced: None,
            _local: Some(db),
        })
    }

    pub async fn open_synced(path: &Path, url: &str, tokens: Arc<dyn TokenSource>) -> Result<Self, StoreError> {
        let file = path_str(path)?;
        let builder = turso::sync::Builder::new_remote(&file)
            .with_remote_url(url)
            .with_client_name("somme")
            .with_auth_token_fn(move || {
                let tokens = Arc::clone(&tokens);
                async move {
                    tokens
                        .token()
                        .await
                        .ok_or_else(|| turso::Error::Misuse("not signed in to Somme Cloud".into()))
                }
            });
        let db = tokio::time::timeout(OPEN_TIMEOUT, builder.build())
            .await
            .map_err(|_| StoreError::Timeout)??;
        let conn = db.connect().await?;
        migrate(&conn).await?;
        Ok(Self {
            path: path.to_owned(),
            conn,
            synced: Some(db),
            _local: None,
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn synced(&self) -> Option<&turso::sync::Database> {
        self.synced.as_ref()
    }

    pub async fn has_vault(&self) -> Result<bool, StoreError> {
        let mut rows = self.conn.query("SELECT COUNT(*) FROM vault_meta", ()).await?;
        let count = match rows.next().await? {
            Some(row) => row.get_value(0)?.as_integer().copied().unwrap_or(0),
            None => 0,
        };
        Ok(count > 0)
    }

    pub async fn copy_from(&self, source: &Connection) -> Result<u64, StoreError> {
        let mut copied = 0;
        for table in DATA_TABLES {
            let mut rows = source.query(format!("SELECT * FROM {table}"), ()).await?;
            let columns = rows.column_names();
            let placeholders = vec!["?"; columns.len()].join(", ");
            let sql = format!(
                "INSERT OR IGNORE INTO {table} ({}) VALUES ({placeholders})",
                columns.join(", ")
            );
            while let Some(row) = rows.next().await? {
                let values = (0..row.column_count())
                    .map(|i| row.get_value(i))
                    .collect::<Result<Vec<Value>, _>>()?;
                copied += self.conn.execute(&sql, values).await?;
            }
        }
        Ok(copied)
    }
}

fn path_str(path: &Path) -> Result<String, StoreError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| StoreError::Path(path.display().to_string()))
}

pub struct Store {
    backend: ArcSwap<Backend>,
    /// Turso fails a query with "concurrent use forbidden" when another task is
    /// using the same connection, so queries take turns.
    turn: Mutex<()>,
    changed: Notify,
}

/// The connection, held until this is dropped. Keep it alive while reading rows.
pub struct Conn<'a> {
    conn: Connection,
    _turn: MutexGuard<'a, ()>,
}

impl Deref for Conn<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl Store {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend: ArcSwap::from_pointee(backend),
            turn: Mutex::new(()),
            changed: Notify::new(),
        }
    }

    pub async fn conn(&self) -> Conn<'_> {
        let turn = self.turn.lock().await;
        Conn {
            conn: self.backend.load().conn.clone(),
            _turn: turn,
        }
    }

    pub fn backend(&self) -> Arc<Backend> {
        self.backend.load_full()
    }

    pub fn replace(&self, backend: Backend) {
        self.backend.store(Arc::new(backend));
    }

    pub fn mark_changed(&self) {
        self.changed.notify_one();
    }

    pub async fn changed(&self) {
        self.changed.notified().await;
    }
}
