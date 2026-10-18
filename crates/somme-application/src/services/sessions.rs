use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use somme_domain::{Id, new_id};

use crate::error::{AppError, AppResult};
use crate::ports::{ConnectContext, OutputSink, PtyGateway, RemoteFs, ShellOptions, SshGateway, TermSize, Terminal};
use crate::services::{ConnectionPlanner, SnippetService};

type Sessions = Arc<DashMap<Id, Arc<dyn Terminal>>>;

struct TrackedSink {
    id: Id,
    inner: Arc<dyn OutputSink>,
    sessions: Sessions,
}

impl OutputSink for TrackedSink {
    fn data(&self, bytes: &[u8]) {
        self.inner.data(bytes);
    }

    fn closed(&self, reason: Option<String>) {
        self.sessions.remove(&self.id);
        self.inner.closed(reason);
    }
}

pub struct SessionService {
    planner: Arc<ConnectionPlanner>,
    snippets: Arc<SnippetService>,
    ssh: Arc<dyn SshGateway>,
    pty: Arc<dyn PtyGateway>,
    ctx: ConnectContext,
    sessions: Sessions,
}

impl SessionService {
    pub fn new(
        planner: Arc<ConnectionPlanner>,
        snippets: Arc<SnippetService>,
        ssh: Arc<dyn SshGateway>,
        pty: Arc<dyn PtyGateway>,
        ctx: ConnectContext,
    ) -> Self {
        Self {
            planner,
            snippets,
            ssh,
            pty,
            ctx,
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Opens a shell on a host. `marks` is the secret for Somme's shell hooks.
    pub async fn open_ssh<F>(&self, host_id: Id, size: TermSize, marks: Option<&str>, make_sink: F) -> AppResult<Id>
    where
        F: FnOnce(Id) -> Arc<dyn OutputSink>,
    {
        let (host, plan) = self.planner.plan(host_id).await?;
        let id = new_id();
        let sink = self.track(id, make_sink(id));
        let terminal = self.ssh.shell(plan, self.ctx.clone(), size, marks, sink).await?;
        self.sessions.insert(id, Arc::clone(&terminal));
        if let Some(snippet_id) = host.startup_snippet_id
            && let Some(snippet) = self.snippets.get(snippet_id).await? {
                terminal.write(format!("{}\n", snippet.script).as_bytes()).await?;
            }
        Ok(id)
    }

    pub fn open_local<F>(&self, size: TermSize, options: ShellOptions<'_>, make_sink: F) -> AppResult<Id>
    where
        F: FnOnce(Id) -> Arc<dyn OutputSink>,
    {
        let id = new_id();
        let sink = self.track(id, make_sink(id));
        let terminal = self.pty.spawn(size, options, sink)?;
        self.sessions.insert(id, terminal);
        Ok(id)
    }

    /// The folder a local shell is in now.
    pub fn cwd(&self, id: Id) -> Option<PathBuf> {
        self.sessions.get(&id)?.cwd()
    }

    /// Whether the session's shell loads Somme's hooks, so marks will come.
    pub fn has_hooks(&self, id: Id) -> bool {
        self.sessions.get(&id).is_some_and(|terminal| terminal.has_hooks())
    }

    /// Files on the server an SSH session runs on.
    pub async fn files(&self, id: Id) -> Option<Arc<dyn RemoteFs>> {
        let terminal = self.get(id).ok()?;
        terminal.files().await
    }

    pub async fn write(&self, id: Id, data: &[u8]) -> AppResult<()> {
        Ok(self.get(id)?.write(data).await?)
    }

    pub async fn resize(&self, id: Id, size: TermSize) -> AppResult<()> {
        Ok(self.get(id)?.resize(size).await?)
    }

    pub async fn close(&self, id: Id) {
        if let Some((_, terminal)) = self.sessions.remove(&id) {
            terminal.close().await;
        }
    }

    pub async fn close_all(&self) {
        let ids: Vec<Id> = self.sessions.iter().map(|e| *e.key()).collect();
        for id in ids {
            self.close(id).await;
        }
    }

    fn get(&self, id: Id) -> AppResult<Arc<dyn Terminal>> {
        self.sessions
            .get(&id)
            .map(|t| Arc::clone(t.value()))
            .ok_or(AppError::NotFound("session"))
    }

    fn track(&self, id: Id, inner: Arc<dyn OutputSink>) -> Arc<dyn OutputSink> {
        Arc::new(TrackedSink {
            id,
            inner,
            sessions: Arc::clone(&self.sessions),
        })
    }
}
