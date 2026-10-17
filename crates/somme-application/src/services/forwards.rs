use std::sync::Arc;

use dashmap::DashMap;
use somme_domain::{ForwardKind, Id, PortForward, Repository, new_id, now};

use crate::dto::{PortForwardInput, PortForwardView};
use crate::error::{AppError, AppResult};
use crate::ports::{ConnectContext, EventPublisher, ForwardSpec, SshGateway, Tunnel};
use crate::services::ConnectionPlanner;
use crate::services::secrets::{clean, clean_opt};

pub struct ForwardService {
    repo: Arc<dyn Repository<PortForward>>,
    planner: Arc<ConnectionPlanner>,
    ssh: Arc<dyn SshGateway>,
    ctx: ConnectContext,
    events: Arc<dyn EventPublisher>,
    active: Arc<DashMap<Id, Arc<dyn Tunnel>>>,
}

impl ForwardService {
    pub fn new(
        repo: Arc<dyn Repository<PortForward>>,
        planner: Arc<ConnectionPlanner>,
        ssh: Arc<dyn SshGateway>,
        ctx: ConnectContext,
        events: Arc<dyn EventPublisher>,
    ) -> Self {
        Self {
            repo,
            planner,
            ssh,
            ctx,
            events,
            active: Arc::new(DashMap::new()),
        }
    }

    pub async fn list(&self) -> AppResult<Vec<PortForwardView>> {
        let mut items = self.repo.list().await?;
        items.sort_by_key(|p| (p.bind_port, p.label.to_lowercase()));
        Ok(items.into_iter().map(Into::into).collect())
    }

    pub async fn save(&self, input: PortForwardInput) -> AppResult<PortForwardView> {
        let stamp = now();
        let existing = match input.id {
            Some(id) => Some(self.repo.get(id).await?.ok_or(AppError::NotFound("port forward"))?),
            None => None,
        };
        let kind: ForwardKind = input.kind.parse()?;
        let dynamic = kind == ForwardKind::Dynamic;
        let rule = PortForward {
            id: existing.as_ref().map_or_else(new_id, |p| p.id),
            label: clean(&input.label),
            host_id: input.host_id,
            kind,
            bind_address: clean(&input.bind_address),
            bind_port: input.bind_port,
            target_host: if dynamic { None } else { clean_opt(input.target_host) },
            target_port: if dynamic { None } else { input.target_port },
            created_at: existing.as_ref().map_or(stamp, |p| p.created_at),
            updated_at: stamp,
        };
        rule.validate()?;
        self.repo.save(&rule).await?;
        Ok(rule.into())
    }

    pub async fn delete(&self, id: Id) -> AppResult<()> {
        self.stop(id).await;
        Ok(self.repo.delete(id).await?)
    }

    pub async fn delete_for_host(&self, host_id: Id) -> AppResult<()> {
        for rule in self.repo.list().await? {
            if rule.host_id == host_id {
                self.delete(rule.id).await?;
            }
        }
        Ok(())
    }

    pub fn active(&self) -> Vec<Id> {
        self.active.iter().map(|e| *e.key()).collect()
    }

    pub async fn start(&self, id: Id) -> AppResult<()> {
        if self.active.contains_key(&id) {
            return Ok(());
        }
        let rule = self.repo.get(id).await?.ok_or(AppError::NotFound("port forward"))?;
        let (_, plan) = self.planner.plan(rule.host_id).await?;
        let spec = ForwardSpec {
            kind: rule.kind,
            bind_address: rule.bind_address.clone(),
            bind_port: rule.bind_port,
            target_host: rule.target_host.clone(),
            target_port: rule.target_port,
        };
        let active = Arc::clone(&self.active);
        let events = Arc::clone(&self.events);
        let on_stop = Box::new(move |reason: Option<String>| {
            active.remove(&id);
            events.forward_stopped(id, reason);
        });
        let tunnel = self.ssh.forward(plan, self.ctx.clone(), spec, on_stop).await?;
        self.active.insert(id, tunnel);
        Ok(())
    }

    pub async fn stop(&self, id: Id) {
        if let Some((_, tunnel)) = self.active.remove(&id) {
            tunnel.stop().await;
        }
    }

    pub async fn stop_all(&self) {
        let ids = self.active();
        for id in ids {
            self.stop(id).await;
        }
    }
}
