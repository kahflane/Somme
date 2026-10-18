use std::path::Path;

use tokio::sync::Mutex;
use turso::{Connection, Value};
use uuid::Uuid;

use crate::domain::{Account, CloudError, GitHubUser};

/// `accounts` held the old email accounts and is no longer read.
const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS github_accounts (
    id TEXT PRIMARY KEY NOT NULL,
    github_id INTEGER NOT NULL UNIQUE,
    login TEXT NOT NULL,
    name TEXT,
    avatar_url TEXT,
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

const COLUMNS: &str = "id, github_id, login, name, avatar_url, database_name, database_host, created_at";

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

fn optional(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::Text)
}

async fn find(conn: &Connection, clause: &str, param: Value) -> Result<Option<Account>, CloudError> {
    let sql = format!("SELECT {COLUMNS} FROM github_accounts WHERE {clause}");
    let mut rows = conn.query(sql, vec![param]).await.map_err(storage)?;
    let Some(row) = rows.next().await.map_err(storage)? else {
        return Ok(None);
    };
    Ok(Some(Account {
        id: text(&row, 0)?.unwrap_or_default().parse().map_err(storage)?,
        github_id: int(&row, 1)?,
        login: text(&row, 2)?.unwrap_or_default(),
        name: text(&row, 3)?,
        avatar_url: text(&row, 4)?,
        database_name: text(&row, 5)?.unwrap_or_default(),
        database_host: text(&row, 6)?,
        created_at: int(&row, 7)?,
    }))
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

    /// Finds the account for a GitHub user, making one the first time they sign
    /// in. Their name and picture are refreshed every time.
    pub async fn sign_in(&self, user: &GitHubUser, now: i64) -> Result<Account, CloudError> {
        let conn = self.conn.lock().await;
        let github_id = Value::Integer(user.id);
        if let Some(mut account) = find(&conn, "github_id = ?", github_id.clone()).await? {
            conn.execute(
                "UPDATE github_accounts SET login = ?, name = ?, avatar_url = ? WHERE id = ?",
                vec![
                    Value::Text(user.login.clone()),
                    optional(&user.name),
                    optional(&user.avatar_url),
                    Value::Text(account.id.to_string()),
                ],
            )
            .await
            .map_err(storage)?;
            account.login.clone_from(&user.login);
            account.name.clone_from(&user.name);
            account.avatar_url.clone_from(&user.avatar_url);
            return Ok(account);
        }
        let id = Uuid::now_v7();
        let account = Account {
            id,
            github_id: user.id,
            login: user.login.clone(),
            name: user.name.clone(),
            avatar_url: user.avatar_url.clone(),
            database_name: Account::database_name_for(id),
            database_host: None,
            created_at: now,
        };
        conn.execute(
            &format!("INSERT INTO github_accounts ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"),
            vec![
                Value::Text(account.id.to_string()),
                github_id,
                Value::Text(account.login.clone()),
                optional(&account.name),
                optional(&account.avatar_url),
                Value::Text(account.database_name.clone()),
                Value::Null,
                Value::Integer(account.created_at),
            ],
        )
        .await
        .map_err(storage)?;
        Ok(account)
    }

    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<Account>, CloudError> {
        find(&*self.conn.lock().await, "id = ?", Value::Text(id.to_string())).await
    }

    pub async fn set_database_host(&self, id: Uuid, host: &str) -> Result<(), CloudError> {
        self.conn
            .lock()
            .await
            .execute(
                "UPDATE github_accounts SET database_host = ? WHERE id = ?",
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

    #[tokio::test]
    async fn the_same_github_user_keeps_one_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path()).await.unwrap();
        let mut user = GitHubUser {
            id: 42,
            login: "octo".into(),
            name: None,
            avatar_url: None,
        };
        let first = store.sign_in(&user, 1).await.unwrap();
        user.login = "octocat".into();
        user.name = Some("The Octocat".into());
        let second = store.sign_in(&user, 2).await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.database_name, second.database_name);
        let saved = store.find_by_id(first.id).await.unwrap().unwrap();
        assert_eq!((saved.login.as_str(), saved.name.as_deref()), ("octocat", Some("The Octocat")));
        assert_eq!(saved.created_at, 1);
    }
}
