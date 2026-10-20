//! Automations in the app: the saved flows, opened from the store, and their
//! runs as they happen. The engine and the run log live in
//! `somme-automation`; `runner` starts runs here and in the background worker.

pub mod assistant;
pub mod builder;
pub mod eval;
pub mod memory;
pub mod skills;
pub mod runner;
pub mod scheduler;
pub mod tools;
pub mod vault;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};
use somme_application::dto::{AutomationInput, AutomationView};
use somme_automation::{Automation, RunRecord, StartedBy};
use somme_domain::Id;

use crate::runtime;
use crate::services::Services;
use crate::store::Store;
use crate::ui::native;
use runner::{Runner, Update};

/// Runs listed per automation.
pub const SHOWN_RUNS: usize = 20;

/// A saved automation, opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub id: Id,
    pub automation: Automation,
    pub updated_at: i64,
}

pub struct Automations {
    pub saved: Vec<Saved>,
    /// Saved ones whose JSON could not be read, like from a newer Somme.
    pub unreadable: Vec<String>,
    /// Runs this session has seen, by run id.
    pub runs: HashMap<String, RunRecord>,
    /// The newest run of each automation, by automation id.
    pub latest: BTreeMap<String, RunRecord>,
    /// The vault values' names, once read. Values are never kept here.
    pub vault_names: Option<Vec<String>>,
    runner: Arc<Runner>,
    views: Vec<AutomationView>,
}

struct GlobalAutomations(Entity<Automations>);

impl Global for GlobalAutomations {}

pub fn init(services: Arc<Services>, cx: &mut App) {
    let (updates, mut incoming) = mpsc::unbounded();
    let (asks, mut asked) = mpsc::unbounded();
    let (notes, mut noted) = mpsc::unbounded::<(String, String)>();
    let (banner, banners) = mpsc::unbounded();
    crate::ui::control_bar::init(banners, cx);
    let runner = Runner::new(services, Some(updates), Some(asks), Some(notes), Some(banner));
    runner::share(runner.clone());
    // Another Somme holds the lock and runs the schedules.
    if crate::mcp::holds_lock() {
        runtime::handle().spawn(scheduler::run(runner.clone()));
    }
    let latest = runner.history().latest();
    let automations = cx.new(|_| Automations {
        saved: Vec::new(),
        unreadable: Vec::new(),
        runs: HashMap::new(),
        latest,
        vault_names: None,
        runner: runner.clone(),
        views: Vec::new(),
    });
    cx.set_global(GlobalAutomations(automations.clone()));
    let weak = automations.downgrade();
    cx.spawn(async move |cx| {
        while let Some(update) = incoming.next().await {
            if weak.update(cx, |automations, cx| automations.apply(update, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some(ask) = asked.next().await {
            cx.update(|cx| crate::ui::guard::ask_user(ask, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some((title, body)) = noted.next().await {
            cx.update(|cx| native::notify(&title, &body, cx));
        }
    })
    .detach();
    let store = Store::global(cx);
    cx.observe(&store, |store, cx| {
        let views = store.read(cx).automations.clone();
        Automations::global(cx).update(cx, |automations, cx| automations.open(views, cx));
    })
    .detach();
    cx.on_app_quit(move |_| {
        runner.cancel_all();
        async {}
    })
    .detach();
}

impl Automations {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAutomations>().0.clone()
    }

    /// Reads the store's automations when they changed.
    fn open(&mut self, views: Vec<AutomationView>, cx: &mut Context<Self>) {
        if views == self.views {
            return;
        }
        self.saved.clear();
        self.unreadable.clear();
        for view in &views {
            match Automation::from_json(&view.document) {
                Ok(mut automation) => {
                    automation.id = view.id.to_string();
                    self.saved.push(Saved { id: view.id, automation, updated_at: view.updated_at });
                }
                Err(err) => self.unreadable.push(err),
            }
        }
        self.views = views;
        // The vault may have just been unlocked.
        self.refresh_vault(cx);
        cx.notify();
    }

    /// Reads the vault values' names again.
    pub fn refresh_vault(&mut self, cx: &mut Context<Self>) {
        let services = Store::services(cx);
        cx.spawn(async move |automations, cx| {
            let names = runtime::io(async move { vault::load(&services).await.map(|values| values.into_keys().collect::<Vec<_>>()) }).await;
            let _ = automations.update(cx, |automations, cx| {
                automations.vault_names = names.ok();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn get(&self, id: &str) -> Option<&Saved> {
        self.saved.iter().find(|saved| saved.id.to_string() == id)
    }

    /// Saves it, new or changed, and gives back its id.
    pub fn save(&self, automation: &Automation, cx: &mut Context<Self>) -> Task<Result<Id, String>> {
        let services = Store::services(cx);
        let id = automation.id.parse::<Id>().ok();
        let mut document = automation.clone();
        // The table's id is the one that counts.
        document.id.clear();
        let input = AutomationInput { id, document: document.to_json() };
        cx.spawn(async move |_, cx| {
            let saved = runtime::io(async move { services.automations.save(input).await }).await;
            cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh(cx)));
            saved.map(|view| view.id).map_err(|err| format!("Could not save the automation: {err}"))
        })
    }

    pub fn delete(&self, id: Id, cx: &mut Context<Self>) {
        let services = Store::services(cx);
        self.runner.history().forget(&id.to_string());
        cx.spawn(async move |_, cx| {
            if let Err(err) = runtime::io(async move { services.automations.delete(id).await }).await {
                tracing::warn!(%err, "could not delete an automation");
            }
            cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh(cx)));
        })
        .detach();
    }

    /// Starts a run by hand and gives its id.
    pub fn run(&mut self, automation: &Automation, inputs: BTreeMap<String, String>) -> Result<String, String> {
        self.runner.start(automation.clone(), inputs, StartedBy::You)
    }

    pub fn cancel(&self, run: &str) {
        self.runner.cancel(run);
    }

    pub fn is_running(&self, run: &str) -> bool {
        self.runner.is_running(run)
    }

    /// Past runs of one automation, newest first, with the live ones.
    pub fn history(&self, automation_id: &str) -> Vec<RunRecord> {
        let mut runs = self.runner.history().runs(automation_id, SHOWN_RUNS);
        for run in runs.iter_mut() {
            if let Some(live) = self.runs.get(&run.head.id) {
                *run = live.clone();
            }
        }
        runs
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Started(head) => {
                let run = RunRecord::new(head);
                self.latest.insert(run.head.automation_id.clone(), run.clone());
                self.runs.insert(run.head.id.clone(), run);
            }
            Update::Event { run, event } => {
                if let Some(record) = self.runs.get_mut(&run) {
                    record.apply(&event);
                    self.latest.insert(record.head.automation_id.clone(), record.clone());
                }
            }
        }
        cx.notify();
    }
}
