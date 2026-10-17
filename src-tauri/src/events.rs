use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::Serialize;
use somme_application::dto::{PromptKindView, SyncStatusView};
use somme_application::ports::{EventPublisher, OutputSink, PromptAnswer, PromptKind, Prompter, SyncState};
use somme_domain::{Id, new_id};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PromptRequest {
    id: Id,
    title: String,
    kind: PromptKindView,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SessionClosed {
    session_id: Id,
    reason: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ForwardStopped {
    rule_id: Id,
    reason: Option<String>,
}

fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(err) = app.emit(event, payload) {
        tracing::warn!(%err, event, "failed to emit event");
    }
}

pub struct TauriEvents {
    app: AppHandle,
}

impl TauriEvents {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventPublisher for TauriEvents {
    fn data_changed(&self) {
        emit(&self.app, "data-changed", ());
    }

    fn sync_status(&self, state: SyncState) {
        emit(&self.app, "sync-status", SyncStatusView::from(state));
    }

    fn forward_stopped(&self, rule_id: Id, reason: Option<String>) {
        emit(&self.app, "forward-stopped", ForwardStopped { rule_id, reason });
    }
}

pub struct TauriPrompter {
    app: AppHandle,
    pending: DashMap<Id, oneshot::Sender<PromptAnswer>>,
}

impl TauriPrompter {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: DashMap::new(),
        }
    }

    pub fn respond(&self, id: Id, answer: PromptAnswer) {
        if let Some((_, sender)) = self.pending.remove(&id) {
            let _ = sender.send(answer);
        }
    }
}

#[async_trait]
impl Prompter for TauriPrompter {
    async fn ask(&self, title: &str, kind: PromptKind) -> PromptAnswer {
        let id = new_id();
        let (sender, receiver) = oneshot::channel();
        self.pending.insert(id, sender);
        emit(
            &self.app,
            "prompt",
            PromptRequest {
                id,
                title: title.to_owned(),
                kind: kind.into(),
            },
        );
        let answer = tokio::time::timeout(PROMPT_TIMEOUT, receiver).await;
        self.pending.remove(&id);
        match answer {
            Ok(Ok(answer)) => answer,
            _ => PromptAnswer::Cancel,
        }
    }
}

pub struct ChannelSink {
    app: AppHandle,
    session_id: Id,
    channel: Channel<InvokeResponseBody>,
}

impl ChannelSink {
    pub fn new(app: AppHandle, session_id: Id, channel: Channel<InvokeResponseBody>) -> Self {
        Self {
            app,
            session_id,
            channel,
        }
    }
}

impl OutputSink for ChannelSink {
    fn data(&self, bytes: &[u8]) {
        if let Err(err) = self.channel.send(InvokeResponseBody::Raw(bytes.to_vec())) {
            tracing::debug!(%err, "terminal channel closed");
        }
    }

    fn closed(&self, reason: Option<String>) {
        emit(
            &self.app,
            "session-closed",
            SessionClosed {
                session_id: self.session_id,
                reason,
            },
        );
    }
}
