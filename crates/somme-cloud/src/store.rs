use std::path::Path;

use tokio::sync::Mutex;
use turso::{Connection, Value};
use uuid::Uuid;

use crate::domain::{Account, CloudError, Email};

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    database_name TEXT NOT NULL,
    database_host TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
";

fn storage(err: impl std::fmt::Display) -> CloudError {
    CloudError::Storage(err.to_string())
}

pub struct AccountStore {
    /// Turso fails a query with "concurrent use forbidden" when another request is
    /// using the same connection, so requests take turns.
    conn: Mutex<Connection>,
    _db: turso::Database,
}

fn text(row: &turso::Row, idx: usize) -> Result<Option<String>, CloudError> {
    match row.get_value(idx).map_err(storage)? {
        Value::Text(s) => Ok(Some(s)),
        Value::Null => Ok(None),
        other => Err(storage(format!("unexpected value {other:?}"))),
    }
}

fn int(row: &turso::Row, idx: usize) -> Result<i64, CloudError> {
    row.get_value(idx)
        .map_err(storage)?
        .as_integer()
        .copied()
        .ok_or_else(|| storage("expected integer"))
}

impl AccountStore {
    pub async fn open(dir: &Path) -> Result<Self, CloudError> {
        std::fs::create_dir_all(dir).map_err(storage)?;
        let path = dir.join("cloud.db");
        let path = path.to_str().ok_or_else(|| storage("data path is not utf-8"))?;
        let db = turso::Builder::new_local(path).build().await.map_err(storage)?;
        let conn = db.connect().map_err(storage)?;
        conn.execute_batch(SCHEMA).await.map_err(storage)?;
        Ok(Self {
            conn: Mutex::new(conn),
            _db: db,
        })
    }

    pub async fn insert_account(&self, account: &Account) -> Result<(), CloudError> {
        self.conn
            .lock()
            .await
            .execute(
                "INSERT INTO accounts (id, email, password_hash, database_name, database_host, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    Value::Text(account.id.to_string()),
                    Value::Text(account.email.as_str().to_owned()),
                    Value::Text(account.password_hash.clone()),
                    Value::Text(account.database_name.clone()),
                    account.database_host.clone().map_or(Value::Null, Value::Text),
                    Value::Integer(account.created_at),
                ],
            )
            .await
            .map_err(|e| match e {
                turso::Error::Constraint(_) => CloudError::EmailTaken,
                other => storage(other),
            })?;
        Ok(())
    }

    async fn find(&self, clause: &str, param: String) -> Result<Option<Account>, CloudError> {
        let sql = format!(
            "SELECT id, email, password_hash, database_name, database_host, created_at FROM accounts WHERE {clause}"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn.query(sql, vec![Value::Text(param)]).await.map_err(storage)?;
        let Some(row) = rows.next().await.map_err(storage)? else {
            return Ok(None);
        };
        let id = text(&row, 0)?.unwrap_or_default();
        Ok(Some(Account {
            id: id.parse().map_err(storage)?,
            email: Email::parse(&text(&row, 1)?.unwrap_or_default())?,
            password_hash: text(&row, 2)?.unwrap_or_default(),
            database_name: text(&row, 3)?.unwrap_or_default(),
            database_host: text(&row, 4)?,
            created_at: int(&row, 5)?,
        }))
    }

    pub async fn find_by_email(&self, email: &Email) -> Result<Option<Account>, CloudError> {
        self.find("email = ?", email.as_str().to_owned()).await
    }

    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<Account>, CloudError> {
        self.find("id = ?", id.to_string()).await
    }

    pub async fn set_database_host(&self, id: Uuid, host: &str) -> Result<(), CloudError> {
        self.conn
            .lock()
            .await
            .execute(
                "UPDATE accounts SET database_host = ? WHERE id = ?",
                vec![Value::Text(host.to_owned()), Value::Text(id.to_string())],
            )
            .await
            .map_err(storage)?;
        Ok(())
    }

    pub async fn insert_session(&self, token_hash: &str, account_id: Uuid, expires_at: i64, now: i64) -> Result<(), CloudError> {
        self.conn
            .lock()
            .await
            .execute(
                "INSERT INTO sessions (token_hash, account_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
                vec![
                    Value::Text(token_hash.to_owned()),
                    Value::Text(account_id.to_string()),
                    Value::Integer(expires_at),
                    Value::Integer(now),
                ],
            )
            .await
            .map_err(storage)?;
        Ok(())
    }

    pub async fn find_session(&self, token_hash: &str) -> Result<Option<(Uuid, i64)>, CloudError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT account_id, expires_at FROM sessions WHERE token_hash = ?",
                vec![Value::Text(token_hash.to_owned())],
            )
            .await
            .map_err(storage)?;
        let Some(row) = rows.next().await.map_err(storage)? else {
            return Ok(None);
        };
        let id = text(&row, 0)?.unwrap_or_default().parse().map_err(storage)?;
        Ok(Some((id, int(&row, 1)?)))
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<(), CloudError> {
        self.conn
            .lock()
            .await
            .execute(
                "DELETE FROM sessions WHERE token_hash = ?",
                vec![Value::Text(token_hash.to_owned())],
            )
            .await
            .map_err(storage)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Requests run at the same time and all share one connection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn overlapping_requests_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(AccountStore::open(dir.path()).await.unwrap());
        let tasks: Vec<_> = (0..32)
            .map(|i| {
                let store = Arc::clone(&store);
                tokio::spawn(async move {
                    let token = format!("token-{i}");
                    store.insert_session(&token, Uuid::new_v4(), 100, 1).await?;
                    store.find_session(&token).await
                })
            })
            .collect();
        for task in tasks {
            assert!(task.await.unwrap().unwrap().is_some());
        }
    }
}
