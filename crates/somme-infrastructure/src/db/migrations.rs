use turso::Connection;

pub const DATA_TABLES: &[&str] = &[
    "vault_meta",
    "groups",
    "ssh_keys",
    "identities",
    "snippets",
    "hosts",
    "port_forwards",
    "known_hosts",
    "preferences",
];

const VAULT_TABLES: &[&str] = &[
    "groups",
    "ssh_keys",
    "identities",
    "snippets",
    "hosts",
    "port_forwards",
    "known_hosts",
];

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS vault_meta (
    id TEXT PRIMARY KEY NOT NULL,
    salt TEXT NOT NULL,
    check_blob TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ssh_keys (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    algorithm TEXT NOT NULL,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    private_key TEXT NOT NULL,
    passphrase TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS identities (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    username TEXT NOT NULL,
    password TEXT,
    key_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS snippets (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    script TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS hosts (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    address TEXT NOT NULL,
    port INTEGER NOT NULL,
    group_id TEXT,
    identity_id TEXT,
    username TEXT,
    password TEXT,
    key_id TEXT,
    jump_host_id TEXT,
    startup_snippet_id TEXT,
    tags TEXT NOT NULL,
    color TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS port_forwards (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    host_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    bind_address TEXT NOT NULL,
    bind_port INTEGER NOT NULL,
    target_host TEXT,
    target_port INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS known_hosts (
    id TEXT PRIMARY KEY NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    key_type TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    public_key TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS preferences (
    id TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
";

pub async fn migrate(conn: &Connection) -> turso::Result<()> {
    conn.execute_batch(SCHEMA).await
}

pub async fn holds_vault_items(conn: &Connection) -> turso::Result<bool> {
    for table in VAULT_TABLES {
        let mut rows = conn.query(format!("SELECT 1 FROM {table} LIMIT 1"), ()).await?;
        if rows.next().await?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
