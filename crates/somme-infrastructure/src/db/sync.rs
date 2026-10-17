use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use somme_application::ports::{EventPublisher, GatewayError, SyncGateway, SyncOutcome, SyncState};
use somme_domain::now;
use tokio::sync::Mutex as AsyncMutex;

use crate::db::config::{DataPaths, SyncConfig};
use crate::db::store::{Backend, Store, StoreError, TokenSource};
const PULL_INTERVAL: Duration = Duration::from_secs(30);
const PUSH_DEBOUNCE: Duration = Duration::from_millis(1500);

fn gateway_error(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Connection(err.to_string())
}

pub struct TursoSync {
    store: Arc<Store>,
    paths: DataPaths,
    tokens: Arc<dyn TokenSource>,
    config: Mutex<SyncConfig>,
    state: Mutex<SyncState>,
    running: AsyncMutex<()>,
    events: OnceLock<Arc<dyn EventPublisher>>,
}

impl TursoSync {
    pub async fn open(paths: DataPaths, tokens: Arc<dyn TokenSource>) -> Result<Arc<Self>, StoreError> {
        let config = SyncConfig::load(&paths);
        let mut last_error = None;
        let backend = match &config.url {
            Some(url) => match Backend::open_synced(&paths.synced_db(url), url, Arc::clone(&tokens)).await {
                Ok(backend) => backend,
                Err(err) => {
                    tracing::error!(%err, "could not open synced database, using local one");
                    last_error = Some(format!("could not open synced database: {err}"));
                    Backend::open_local(&paths.local_db()).await?
                }
            },
            None => Backend::open_local(&paths.local_db()).await?,
        };
        let enabled = config.enabled && backend.synced().is_some();
        let state = SyncState {
            enabled,
            url: config.url.clone(),
            syncing: false,
            last_sync_at: None,
            last_error,
        };
        Ok(Arc::new(Self {
            store: Arc::new(Store::new(backend)),
            paths,
            tokens,
            config: Mutex::new(config),
            state: Mutex::new(state),
            running: AsyncMutex::new(()),
            events: OnceLock::new(),
        }))
    }

    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    pub fn start(self: &Arc<Self>, events: Arc<dyn EventPublisher>) {
        let _ = self.events.set(events);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(PULL_INTERVAL) => {}
                    () = this.store.changed() => tokio::time::sleep(PUSH_DEBOUNCE).await,
                }
                if this.state().enabled
                    && let Err(err) = this.run().await {
                        tracing::warn!(%err, "background sync failed");
                    }
            }
        });
    }

    async fn run(&self) -> Result<(), GatewayError> {
        let _guard = self.running.lock().await;
        let backend = self.store.backend();
        let Some(db) = backend.synced() else {
            return Err(GatewayError::Connection("sync is not configured".into()));
        };
        self.update(|s| s.syncing = true);
        let result = async {
            db.push().await?;
            db.pull().await
        }
        .await;
        match result {
            Ok(changed) => {
                self.update(|s| {
                    s.syncing = false;
                    s.last_sync_at = Some(now());
                    s.last_error = None;
                });
                if changed
                    && let Some(events) = self.events.get() {
                        events.data_changed();
                    }
                Ok(())
            }
            Err(err) => {
                self.update(|s| {
                    s.syncing = false;
                    s.last_error = Some(err.to_string());
                });
                Err(gateway_error(err))
            }
        }
    }

    fn update(&self, change: impl FnOnce(&mut SyncState)) {
        let snapshot = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            change(&mut state);
            state.clone()
        };
        if let Some(events) = self.events.get() {
            events.sync_status(snapshot);
        }
    }

    fn save_config(&self, config: SyncConfig) -> Result<(), GatewayError> {
        config.save(&self.paths).map_err(|e| GatewayError::Io(e.to_string()))?;
        *self.config.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = config;
        Ok(())
    }

    fn current_url(&self) -> Option<String> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .url
            .clone()
    }
}

#[async_trait]
impl SyncGateway for TursoSync {
    fn state(&self) -> SyncState {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn connect(&self, url: &str) -> Result<SyncOutcome, GatewayError> {
        let guard = self.running.lock().await;
        let current = self.store.backend();
        let same_target = current.synced().is_some() && self.current_url().as_deref() == Some(url);
        let outcome = if same_target {
            SyncOutcome::RemoteKept
        } else {
            let next = Backend::open_synced(&self.paths.synced_db(url), url, Arc::clone(&self.tokens))
                .await
                .map_err(gateway_error)?;
            let outcome = if next.has_vault().await.map_err(gateway_error)? {
                SyncOutcome::RemoteKept
            } else {
                next.copy_from(&current).await.map_err(gateway_error)?;
                SyncOutcome::Merged
            };
            if let Some(db) = next.synced() {
                db.push().await.map_err(gateway_error)?;
            }
            self.store.replace(next);
            outcome
        };
        self.save_config(SyncConfig {
            url: Some(url.to_owned()),
            enabled: true,
        })?;
        self.update(|s| {
            s.enabled = true;
            s.url = Some(url.to_owned());
            s.last_error = None;
        });
        drop(guard);
        self.run().await?;
        Ok(outcome)
    }

    async fn disconnect(&self) -> Result<(), GatewayError> {
        let _guard = self.running.lock().await;
        self.save_config(SyncConfig {
            url: self.current_url(),
            enabled: false,
        })?;
        self.update(|s| {
            s.enabled = false;
            s.syncing = false;
        });
        Ok(())
    }

    async fn sync_now(&self) -> Result<(), GatewayError> {
        if !self.state().enabled {
            return Err(GatewayError::Connection("sync is not enabled".into()));
        }
        self.run().await
    }
}
