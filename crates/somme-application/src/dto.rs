use serde::{Deserialize, Serialize};
use somme_domain::{Group, Host, Id, Identity, KnownHost, PortForward, Snippet, SshKey};

use crate::ports::{KeyboardPrompt, PromptAnswer, PromptKind, RemoteEntry, SyncState};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "camelCase")]
pub enum SecretUpdate {
    Keep,
    Clear,
    Set(String),
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub initialized: bool,
    pub unlocked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub id: Id,
    pub name: String,
    pub parent_id: Option<Id>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Group> for GroupView {
    fn from(g: Group) -> Self {
        Self {
            id: g.id,
            name: g.name,
            parent_id: g.parent_id,
            created_at: g.created_at,
            updated_at: g.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInput {
    pub id: Option<Id>,
    pub name: String,
    pub parent_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub id: Id,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub group_id: Option<Id>,
    pub identity_id: Option<Id>,
    pub username: Option<String>,
    pub has_password: bool,
    pub key_id: Option<Id>,
    pub jump_host_id: Option<Id>,
    pub startup_snippet_id: Option<Id>,
    pub tags: Vec<String>,
    pub color: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Host> for HostView {
    fn from(h: Host) -> Self {
        Self {
            has_password: h.password.is_some(),
            id: h.id,
            label: h.label,
            address: h.address,
            port: h.port,
            group_id: h.group_id,
            identity_id: h.identity_id,
            username: h.username,
            key_id: h.key_id,
            jump_host_id: h.jump_host_id,
            startup_snippet_id: h.startup_snippet_id,
            tags: h.tags,
            color: h.color,
            created_at: h.created_at,
            updated_at: h.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInput {
    pub id: Option<Id>,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub group_id: Option<Id>,
    pub identity_id: Option<Id>,
    pub username: Option<String>,
    pub password: SecretUpdate,
    pub key_id: Option<Id>,
    pub jump_host_id: Option<Id>,
    pub startup_snippet_id: Option<Id>,
    pub tags: Vec<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub id: Id,
    pub label: String,
    pub username: String,
    pub has_password: bool,
    pub key_id: Option<Id>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Identity> for IdentityView {
    fn from(i: Identity) -> Self {
        Self {
            has_password: i.password.is_some(),
            id: i.id,
            label: i.label,
            username: i.username,
            key_id: i.key_id,
            created_at: i.created_at,
            updated_at: i.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityInput {
    pub id: Option<Id>,
    pub label: String,
    pub username: String,
    pub password: SecretUpdate,
    pub key_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub id: Id,
    pub label: String,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
    pub has_passphrase: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<SshKey> for KeyView {
    fn from(k: SshKey) -> Self {
        Self {
            has_passphrase: k.passphrase.is_some(),
            id: k.id,
            label: k.label,
            algorithm: k.algorithm,
            public_key: k.public_key,
            fingerprint: k.fingerprint,
            created_at: k.created_at,
            updated_at: k.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyImportInput {
    pub label: String,
    pub private_key: String,
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyGenerateInput {
    pub label: String,
    pub algorithm: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetView {
    pub id: Id,
    pub label: String,
    pub script: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Snippet> for SnippetView {
    fn from(s: Snippet) -> Self {
        Self {
            id: s.id,
            label: s.label,
            script: s.script,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetInput {
    pub id: Option<Id>,
    pub label: String,
    pub script: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortForwardView {
    pub id: Id,
    pub label: String,
    pub host_id: Id,
    pub kind: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<PortForward> for PortForwardView {
    fn from(p: PortForward) -> Self {
        Self {
            id: p.id,
            label: p.label,
            host_id: p.host_id,
            kind: p.kind.as_str().to_owned(),
            bind_address: p.bind_address,
            bind_port: p.bind_port,
            target_host: p.target_host,
            target_port: p.target_port,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortForwardInput {
    pub id: Option<Id>,
    pub label: String,
    pub host_id: Id,
    pub kind: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostView {
    pub id: Id,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub created_at: i64,
}

impl From<KnownHost> for KnownHostView {
    fn from(k: KnownHost) -> Self {
        Self {
            id: k.id,
            host: k.host,
            port: k.port,
            key_type: k.key_type,
            fingerprint: k.fingerprint,
            created_at: k.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntryView {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified_at: Option<i64>,
    pub permissions: Option<String>,
}

impl From<RemoteEntry> for SftpEntryView {
    fn from(e: RemoteEntry) -> Self {
        Self {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            is_symlink: e.is_symlink,
            size: e.size,
            modified_at: e.modified_at,
            permissions: e.permissions,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpOpened {
    pub sftp_id: Id,
    pub home: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub signed_in: bool,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusView {
    pub enabled: bool,
    pub url: Option<String>,
    pub syncing: bool,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
}

impl From<SyncState> for SyncStatusView {
    fn from(s: SyncState) -> Self {
        Self {
            enabled: s.enabled,
            url: s.url,
            syncing: s.syncing,
            last_sync_at: s.last_sync_at,
            last_error: s.last_error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardPromptView {
    pub text: String,
    pub echo: bool,
}

impl From<KeyboardPrompt> for KeyboardPromptView {
    fn from(p: KeyboardPrompt) -> Self {
        Self {
            text: p.text,
            echo: p.echo,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PromptKindView {
    Password {
        username: String,
        host: String,
    },
    Passphrase {
        key_label: String,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<KeyboardPromptView>,
    },
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        previous_fingerprint: Option<String>,
    },
}

impl From<PromptKind> for PromptKindView {
    fn from(kind: PromptKind) -> Self {
        match kind {
            PromptKind::Password { username, host } => Self::Password { username, host },
            PromptKind::Passphrase { key_label } => Self::Passphrase { key_label },
            PromptKind::KeyboardInteractive {
                name,
                instructions,
                prompts,
            } => Self::KeyboardInteractive {
                name,
                instructions,
                prompts: prompts.into_iter().map(Into::into).collect(),
            },
            PromptKind::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                previous_fingerprint,
            } => Self::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                previous_fingerprint,
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptResponse {
    Answers { answers: Vec<String> },
    Accept,
    Cancel,
}

impl From<PromptResponse> for PromptAnswer {
    fn from(r: PromptResponse) -> Self {
        match r {
            PromptResponse::Answers { answers } => Self::Answers(answers),
            PromptResponse::Accept => Self::Accept,
            PromptResponse::Cancel => Self::Cancel,
        }
    }
}
