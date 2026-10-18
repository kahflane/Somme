use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use somme_domain::{Id, KnownHostRepository};

use crate::error::AppResult;
use crate::ports::{ConnectContext, PromptAnswer, PromptKind, Prompter, ScriptOutput, SshGateway};
use crate::services::{ConnectionPlanner, KnownHostsVerifier};

/// Answers no question. Scripts run for agents while nobody may be watching,
/// so a server that wants a typed password or a new host key is refused.
struct NoPrompts;

#[async_trait]
impl Prompter for NoPrompts {
    async fn ask(&self, _title: &str, _kind: PromptKind) -> PromptAnswer {
        PromptAnswer::Cancel
    }
}

/// Runs scripts on saved hosts without a terminal, for agents.
pub struct ScriptService {
    planner: Arc<ConnectionPlanner>,
    ssh: Arc<dyn SshGateway>,
    ctx: ConnectContext,
}

impl ScriptService {
    pub fn new(planner: Arc<ConnectionPlanner>, ssh: Arc<dyn SshGateway>, known_hosts: Arc<dyn KnownHostRepository>) -> Self {
        let prompter: Arc<dyn Prompter> = Arc::new(NoPrompts);
        let ctx = ConnectContext {
            verifier: Arc::new(KnownHostsVerifier::new(known_hosts, prompter.clone())),
            prompter,
        };
        Self { planner, ssh, ctx }
    }

    /// Only hosts whose key was already trusted in Somme connect, and only
    /// with saved passwords and keys.
    pub async fn run(&self, host_id: Id, script: &str, timeout: Duration, limit: usize) -> AppResult<ScriptOutput> {
        let (_, plan) = self.planner.plan(host_id).await?;
        Ok(self.ssh.run_script(plan, self.ctx.clone(), script, timeout, limit).await?)
    }
}
