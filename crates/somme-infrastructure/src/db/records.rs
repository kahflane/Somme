use somme_domain::{
    Group, Host, Id, Identity, KnownHost, PortForward, RepositoryError, Sealed, Snippet, SshKey,
};
use turso::{Row, Value};

pub trait SqlEntity: Sized + Send + Sync + 'static {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];

    fn id(&self) -> Id;
    fn values(&self) -> Vec<Value>;
    fn from_row(row: &RowReader<'_>) -> Result<Self, RepositoryError>;
}

pub struct RowReader<'a> {
    table: &'static str,
    row: &'a Row,
}

impl<'a> RowReader<'a> {
    pub fn new(table: &'static str, row: &'a Row) -> Self {
        Self { table, row }
    }

    fn corrupt(&self, reason: impl Into<String>) -> RepositoryError {
        RepositoryError::Corrupt {
            table: self.table,
            reason: reason.into(),
        }
    }

    fn value(&self, idx: usize) -> Result<Value, RepositoryError> {
        self.row.get_value(idx).map_err(|e| self.corrupt(e.to_string()))
    }

    pub fn opt_text(&self, idx: usize) -> Result<Option<String>, RepositoryError> {
        match self.value(idx)? {
            Value::Null => Ok(None),
            Value::Text(s) => Ok(Some(s)),
            other => Err(self.corrupt(format!("column {idx}: expected text, got {other:?}"))),
        }
    }

    pub fn text(&self, idx: usize) -> Result<String, RepositoryError> {
        self.opt_text(idx)?
            .ok_or_else(|| self.corrupt(format!("column {idx}: unexpected null")))
    }

    pub fn opt_int(&self, idx: usize) -> Result<Option<i64>, RepositoryError> {
        match self.value(idx)? {
            Value::Null => Ok(None),
            Value::Integer(i) => Ok(Some(i)),
            other => Err(self.corrupt(format!("column {idx}: expected integer, got {other:?}"))),
        }
    }

    pub fn int(&self, idx: usize) -> Result<i64, RepositoryError> {
        self.opt_int(idx)?
            .ok_or_else(|| self.corrupt(format!("column {idx}: unexpected null")))
    }

    pub fn opt_port(&self, idx: usize) -> Result<Option<u16>, RepositoryError> {
        self.opt_int(idx)?
            .map(|p| u16::try_from(p).map_err(|_| self.corrupt(format!("invalid port {p}"))))
            .transpose()
    }

    pub fn port(&self, idx: usize) -> Result<u16, RepositoryError> {
        self.opt_port(idx)?
            .ok_or_else(|| self.corrupt(format!("column {idx}: unexpected null")))
    }

    pub fn opt_id(&self, idx: usize) -> Result<Option<Id>, RepositoryError> {
        self.opt_text(idx)?
            .map(|s| s.parse().map_err(|_| self.corrupt(format!("invalid id {s}"))))
            .transpose()
    }

    pub fn id(&self, idx: usize) -> Result<Id, RepositoryError> {
        self.opt_id(idx)?
            .ok_or_else(|| self.corrupt(format!("column {idx}: unexpected null")))
    }

    pub fn opt_sealed(&self, idx: usize) -> Result<Option<Sealed>, RepositoryError> {
        Ok(self.opt_text(idx)?.map(Sealed::new))
    }

    pub fn sealed(&self, idx: usize) -> Result<Sealed, RepositoryError> {
        Ok(Sealed::new(self.text(idx)?))
    }
}

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn opt_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

fn id(value: Id) -> Value {
    Value::Text(value.to_string())
}

fn opt_id(value: Option<Id>) -> Value {
    value.map_or(Value::Null, id)
}

fn sealed(value: Option<&Sealed>) -> Value {
    opt_text(value.map(Sealed::as_str))
}

fn int(value: impl Into<i64>) -> Value {
    Value::Integer(value.into())
}

fn opt_int(value: Option<impl Into<i64>>) -> Value {
    value.map_or(Value::Null, int)
}

impl SqlEntity for Group {
    const TABLE: &'static str = "groups";
    const COLUMNS: &'static [&'static str] = &["id", "name", "parent_id", "created_at", "updated_at"];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.name),
            opt_id(self.parent_id),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        Ok(Self {
            id: r.id(0)?,
            name: r.text(1)?,
            parent_id: r.opt_id(2)?,
            created_at: r.int(3)?,
            updated_at: r.int(4)?,
        })
    }
}

impl SqlEntity for Host {
    const TABLE: &'static str = "hosts";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "label",
        "address",
        "port",
        "group_id",
        "identity_id",
        "username",
        "password",
        "key_id",
        "jump_host_id",
        "startup_snippet_id",
        "tags",
        "color",
        "created_at",
        "updated_at",
    ];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.label),
            text(&self.address),
            int(self.port),
            opt_id(self.group_id),
            opt_id(self.identity_id),
            opt_text(self.username.as_deref()),
            sealed(self.password.as_ref()),
            opt_id(self.key_id),
            opt_id(self.jump_host_id),
            opt_id(self.startup_snippet_id),
            text(&serde_json::to_string(&self.tags).unwrap_or_else(|_| "[]".into())),
            opt_text(self.color.as_deref()),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        let tags = serde_json::from_str(&r.text(11)?).unwrap_or_default();
        Ok(Self {
            id: r.id(0)?,
            label: r.text(1)?,
            address: r.text(2)?,
            port: r.port(3)?,
            group_id: r.opt_id(4)?,
            identity_id: r.opt_id(5)?,
            username: r.opt_text(6)?,
            password: r.opt_sealed(7)?,
            key_id: r.opt_id(8)?,
            jump_host_id: r.opt_id(9)?,
            startup_snippet_id: r.opt_id(10)?,
            tags,
            color: r.opt_text(12)?,
            created_at: r.int(13)?,
            updated_at: r.int(14)?,
        })
    }
}

impl SqlEntity for Identity {
    const TABLE: &'static str = "identities";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "label",
        "username",
        "password",
        "key_id",
        "created_at",
        "updated_at",
    ];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.label),
            text(&self.username),
            sealed(self.password.as_ref()),
            opt_id(self.key_id),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        Ok(Self {
            id: r.id(0)?,
            label: r.text(1)?,
            username: r.text(2)?,
            password: r.opt_sealed(3)?,
            key_id: r.opt_id(4)?,
            created_at: r.int(5)?,
            updated_at: r.int(6)?,
        })
    }
}

impl SqlEntity for SshKey {
    const TABLE: &'static str = "ssh_keys";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "label",
        "algorithm",
        "public_key",
        "fingerprint",
        "private_key",
        "passphrase",
        "created_at",
        "updated_at",
    ];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.label),
            text(&self.algorithm),
            text(&self.public_key),
            text(&self.fingerprint),
            text(self.private_key.as_str()),
            sealed(self.passphrase.as_ref()),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        Ok(Self {
            id: r.id(0)?,
            label: r.text(1)?,
            algorithm: r.text(2)?,
            public_key: r.text(3)?,
            fingerprint: r.text(4)?,
            private_key: r.sealed(5)?,
            passphrase: r.opt_sealed(6)?,
            created_at: r.int(7)?,
            updated_at: r.int(8)?,
        })
    }
}

impl SqlEntity for Snippet {
    const TABLE: &'static str = "snippets";
    const COLUMNS: &'static [&'static str] = &["id", "label", "script", "created_at", "updated_at"];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.label),
            text(&self.script),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        Ok(Self {
            id: r.id(0)?,
            label: r.text(1)?,
            script: r.text(2)?,
            created_at: r.int(3)?,
            updated_at: r.int(4)?,
        })
    }
}

impl SqlEntity for PortForward {
    const TABLE: &'static str = "port_forwards";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "label",
        "host_id",
        "kind",
        "bind_address",
        "bind_port",
        "target_host",
        "target_port",
        "created_at",
        "updated_at",
    ];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.label),
            id(self.host_id),
            text(self.kind.as_str()),
            text(&self.bind_address),
            int(self.bind_port),
            opt_text(self.target_host.as_deref()),
            opt_int(self.target_port),
            int(self.created_at),
            int(self.updated_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        let kind = r.text(3)?.parse().map_err(|e: somme_domain::DomainError| {
            RepositoryError::Corrupt {
                table: Self::TABLE,
                reason: e.to_string(),
            }
        })?;
        Ok(Self {
            id: r.id(0)?,
            label: r.text(1)?,
            host_id: r.id(2)?,
            kind,
            bind_address: r.text(4)?,
            bind_port: r.port(5)?,
            target_host: r.opt_text(6)?,
            target_port: r.opt_port(7)?,
            created_at: r.int(8)?,
            updated_at: r.int(9)?,
        })
    }
}

impl SqlEntity for KnownHost {
    const TABLE: &'static str = "known_hosts";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "host",
        "port",
        "key_type",
        "fingerprint",
        "public_key",
        "created_at",
    ];

    fn id(&self) -> Id {
        self.id
    }

    fn values(&self) -> Vec<Value> {
        vec![
            id(self.id),
            text(&self.host),
            int(self.port),
            text(&self.key_type),
            text(&self.fingerprint),
            text(&self.public_key),
            int(self.created_at),
        ]
    }

    fn from_row(r: &RowReader<'_>) -> Result<Self, RepositoryError> {
        Ok(Self {
            id: r.id(0)?,
            host: r.text(1)?,
            port: r.port(2)?,
            key_type: r.text(3)?,
            fingerprint: r.text(4)?,
            public_key: r.text(5)?,
            created_at: r.int(6)?,
        })
    }
}
