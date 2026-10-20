//! Starts and stops runs and writes their history. No GPUI, so the app and
//! the background worker each hold one.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use futures::channel::mpsc::UnboundedSender;
use parking_lot::Mutex;
use somme_automation::schema::problems;
use somme_automation::{Automation, Event, History, RunHead, StartedBy};
use tokio_util::sync::CancellationToken;

use super::tools::AppTools;
use crate::ai::GuardAsk;
use crate::computer::Banner;
use crate::runtime;
use crate::services::Services;

/// What a run tells whoever watches.
pub enum Update {
    Started(RunHead),
    Event { run: String, event: Event },
}

/// The runner agents reach, in the app or in the background worker.
static SHARED: OnceLock<Arc<Runner>> = OnceLock::new();

pub fn share(runner: Arc<Runner>) {
    let _ = SHARED.set(runner);
}

pub fn shared() -> Option<Arc<Runner>> {
    SHARED.get().cloned()
}

pub struct Runner {
    services: Arc<Services>,
    history: History,
    /// Each live run's automation and its stop.
    live: Mutex<HashMap<String, (String, CancellationToken)>>,
    updates: Option<UnboundedSender<Update>>,
    asks: Option<UnboundedSender<GuardAsk>>,
    notes: Option<UnboundedSender<(String, String)>>,
    /// Where the bar for an agent using the screen goes. None in the background worker.
    banner: Option<UnboundedSender<Banner>>,
}

impl Runner {
    pub fn new(
        services: Arc<Services>,
        updates: Option<UnboundedSender<Update>>,
        asks: Option<UnboundedSender<GuardAsk>>,
        notes: Option<UnboundedSender<(String, String)>>,
        banner: Option<UnboundedSender<Banner>>,
    ) -> Arc<Self> {
        let history = History::new(services.data_dir.join("automations").join("runs"));
        Arc::new(Self { services, history, live: Mutex::new(HashMap::new()), updates, asks, notes, banner })
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn is_running(&self, run: &str) -> bool {
        self.live.lock().contains_key(run)
    }

    /// Whether a run of this automation is going.
    pub fn is_automation_running(&self, automation_id: &str) -> bool {
        self.live.lock().values().any(|(automation, _)| automation == automation_id)
    }

    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    /// Starts a run on the IO runtime and gives its id.
    pub fn start(self: &Arc<Self>, automation: Automation, inputs: BTreeMap<String, String>, by: StartedBy) -> Result<String, String> {
        if let Some(problem) = problems(&automation).into_iter().next() {
            return Err(problem);
        }
        let head = RunHead {
            id: uuid::Uuid::now_v7().to_string(),
            automation_id: automation.id.clone(),
            automation_name: automation.name.clone(),
            by,
            inputs: inputs.clone(),
        };
        // No log, no run.
        let writer = self.history.start(&head).map_err(|err| format!("Could not write the run log: {err}"))?;
        let cancel = CancellationToken::new();
        self.live.lock().insert(head.id.clone(), (automation.id.clone(), cancel.clone()));
        self.send(Update::Started(head.clone()));
        let this = self.clone();
        let id = head.id.clone();
        runtime::handle().spawn(async move {
            let tools = Arc::new(AppTools {
                services: this.services.clone(),
                who: format!("Automation \u{201c}{}\u{201d}", automation.name),
                asks: this.asks.clone(),
                notes: this.notes.clone(),
                banner: this.banner.clone(),
                screen: Default::default(),
                ai_calls: Default::default(),
                secrets: Default::default(),
                chat: None,
                cleared: Default::default(),
                automation: (!automation.id.is_empty()).then(|| automation.id.clone()),
            });
            let on_event = |event: Event| {
                writer.write(&event);
                this.send(Update::Event { run: head.id.clone(), event });
            };
            somme_automation::run(&automation, inputs, tools, &on_event, cancel).await;
            this.live.lock().remove(&head.id);
        });
        Ok(id)
    }

    pub fn cancel(&self, run: &str) {
        if let Some((_, cancel)) = self.live.lock().get(run) {
            cancel.cancel();
        }
    }

    /// Stops every run, as when Somme quits.
    pub fn cancel_all(&self) {
        for (_, cancel) in self.live.lock().values() {
            cancel.cancel();
        }
    }

    fn send(&self, update: Update) {
        if let Some(updates) = &self.updates {
            let _ = updates.unbounded_send(update);
        }
    }
}
