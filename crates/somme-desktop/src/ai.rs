//! The AI features as Somme runs them: the keys, sealed with the vault in the
//! synced database, and the clients built from them. Every feature checks the
//! master switch and its own switch first, so turning AI off stops all of it.
//! Works in the app and in the background worker alike.

use std::time::Duration;

use futures::channel::oneshot;
use somme_ai::clean::Cleaner;
use somme_ai::decide::{Asker, LlmAsker};
use somme_ai::guard::{self, Action, Decision, Verdict};
use somme_ai::jev::Jev;
use somme_ai::llm::{Llm, LlmConfig};
use somme_domain::Sealed;

use crate::services::Services;
use crate::settings::Settings;

/// Where the sealed keys are kept, in the synced preferences.
const LLM_KEY: &str = "ai.llmKey";
const JEV_KEY: &str = "ai.jevKey";

/// Which key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyName {
    Llm,
    Jev,
}

impl KeyName {
    fn preference(self) -> &'static str {
        match self {
            Self::Llm => LLM_KEY,
            Self::Jev => JEV_KEY,
        }
    }
}

/// One AI feature, each with its own switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Guard,
    Decide,
    Write,
    Builder,
    Agent,
    Computer,
}

/// Whether `feature` may run: AI is on and the feature is on.
pub fn allowed(settings: &Settings, feature: Feature) -> bool {
    let ai = &settings.ai;
    ai.on
        && match feature {
            Feature::Guard => ai.guard,
            Feature::Decide => ai.decide,
            Feature::Write => ai.write,
            Feature::Builder => ai.builder,
            Feature::Agent => ai.agent,
            Feature::Computer => ai.agent && ai.computer,
        }
}

/// The keys, opened. A key is missing when it is not saved or the vault is locked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub llm: Option<String>,
    pub jev: Option<String>,
}

async fn key(services: &Services, name: KeyName) -> Option<String> {
    let stored = services.preferences.get(name.preference()).await.ok().flatten()?;
    if stored.is_empty() {
        return None;
    }
    let opened = services.vault.open(&Sealed::new(stored)).ok()?;
    Some(opened.to_string()).filter(|key| !key.trim().is_empty())
}

pub async fn keys(services: &Services) -> Keys {
    Keys { llm: key(services, KeyName::Llm).await, jev: key(services, KeyName::Jev).await }
}

/// Whether a key is saved, without opening it.
pub async fn has_key(services: &Services, name: KeyName) -> bool {
    services.preferences.get(name.preference()).await.ok().flatten().is_some_and(|stored| !stored.is_empty())
}

/// Saves a key sealed with the vault, or removes it when `value` is empty.
pub async fn save_key(services: &Services, name: KeyName, value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    let stored = if value.is_empty() { String::new() } else { services.vault.seal(value)?.into_inner() };
    services.preferences.set(name.preference(), &stored).await?;
    Ok(())
}

/// Which keys are saved, for screens that can't wait to ask.
#[derive(Clone, Copy, Debug, Default)]
pub struct SavedKeys {
    pub llm: bool,
    pub jev: bool,
}

impl gpui_kit::Global for SavedKeys {}

/// Looks up which keys are saved, then redraws.
pub fn check_saved_keys(services: std::sync::Arc<Services>, cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        let saved = crate::runtime::io(async move {
            SavedKeys { llm: has_key(&services, KeyName::Llm).await, jev: has_key(&services, KeyName::Jev).await }
        })
        .await;
        cx.update(|cx| {
            cx.set_global(saved);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Whether AI is on with something to ask: an LLM with an address, a model
/// and a key (none for one on this computer), or a Jev key.
pub fn is_configured(settings: &Settings, cx: &gpui_kit::App) -> bool {
    let saved = cx.try_global::<SavedKeys>().copied().unwrap_or_default();
    let ai = &settings.ai;
    let llm = !ai.llm_base_url.trim().is_empty()
        && !ai.llm_model.trim().is_empty()
        && (saved.llm || somme_ai::llm::is_local(&ai.llm_base_url));
    ai.on && (llm || saved.jev)
}

/// The LLM settings with its key, when an address and model are set.
pub fn llm_config(settings: &Settings, keys: &Keys) -> Option<LlmConfig> {
    llm_config_for(settings, keys, Feature::Guard)
}

/// The model picked for `feature`, else the main one.
pub fn model_for(settings: &Settings, feature: Feature) -> &str {
    let ai = &settings.ai;
    let own = match feature {
        Feature::Builder => &ai.builder_model,
        Feature::Decide => &ai.decide_model,
        Feature::Write => &ai.write_model,
        Feature::Agent | Feature::Computer => &ai.agent_model,
        Feature::Guard => "",
    };
    if own.trim().is_empty() { ai.llm_model.trim() } else { own.trim() }
}

/// `llm_config` with the model picked for `feature`.
pub fn llm_config_for(settings: &Settings, keys: &Keys, feature: Feature) -> Option<LlmConfig> {
    let config = LlmConfig {
        base_url: settings.ai.llm_base_url.trim().to_owned(),
        api_key: keys.llm.clone(),
        model: model_for(settings, feature).to_owned(),
    };
    config.is_set_up().then_some(config)
}

/// The clients for one use, built from the settings and keys.
pub struct Clients {
    pub jev: Option<Jev>,
    pub llm: Option<Llm>,
    llm_asker: Option<LlmAsker>,
    pub cleaner: Cleaner,
}

impl Clients {
    /// The clients, with the LLM model picked for `feature`.
    pub async fn load(services: &Services, settings: &Settings, feature: Feature) -> Self {
        let keys = keys(services).await;
        let jev = keys.jev.clone().map(|key| Jev::new(key, settings.ai.jev_model.clone()));
        let config = llm_config_for(settings, &keys, feature);
        let llm = config.as_ref().map(Llm::new);
        let llm_asker = config.as_ref().map(|config| LlmAsker::new(Llm::new(config)));
        let cleaner = Cleaner::new(saved_secrets(services, &keys).await);
        Self { jev, llm, llm_asker, cleaner }
    }

    /// Jev, then the LLM, for `feature`. Empty when AI or the feature is off.
    pub fn askers(&self, settings: &Settings, feature: Feature) -> Vec<&dyn Asker> {
        if !allowed(settings, feature) {
            return Vec::new();
        }
        let mut askers: Vec<&dyn Asker> = Vec::new();
        if let Some(jev) = &self.jev {
            askers.push(jev);
        }
        if let Some(llm) = &self.llm_asker {
            askers.push(llm);
        }
        askers
    }

    /// The LLM for `feature`, when AI and the feature are on.
    pub fn llm_for(&self, settings: &Settings, feature: Feature) -> Option<&Llm> {
        allowed(settings, feature).then_some(self.llm.as_ref()).flatten()
    }
}

/// Saved passwords and the AI keys themselves, to hide them exactly.
async fn saved_secrets(services: &Services, keys: &Keys) -> Vec<String> {
    let mut secrets: Vec<String> = [keys.llm.clone(), keys.jev.clone()].into_iter().flatten().collect();
    if let Ok(hosts) = services.hosts.all().await {
        for password in hosts.iter().filter_map(|host| host.password.as_ref()) {
            if let Ok(opened) = services.vault.open(password) {
                secrets.push(opened.to_string());
            }
        }
    }
    if let Ok(values) = crate::automations::vault::load(services).await {
        secrets.extend(values.into_values());
    }
    secrets
}

/// Checks an action against the guard rules. Reads the settings from disk,
/// so the app and the background worker check the same way.
pub async fn guard(services: &Services, action: Action) -> Verdict {
    let settings = Settings::load(&services.data_dir);
    guard_with(services, &settings, action).await
}

pub async fn guard_with(services: &Services, settings: &Settings, mut action: Action) -> Verdict {
    if settings.guard_rules.is_empty() {
        return guard::check(&[], settings.guard_strictness, &action, &[]).await;
    }
    // A command that only looks gets the quick check: an AI call for each
    // look made agents slow. What the quick check doubts still goes to AI.
    if action.kind == "command" && somme_automation::readonly::check(&action.detail).is_ok() {
        let quick = guard::check(&settings.guard_rules, settings.guard_strictness, &action, &[]).await;
        if quick.decision != Decision::Ask {
            return quick;
        }
    }
    let clients = Clients::load(services, settings, Feature::Guard).await;
    action.detail = clients.cleaner.clean(&action.detail);
    let askers = clients.askers(settings, Feature::Guard);
    guard::check(&settings.guard_rules, settings.guard_strictness, &action, &askers).await
}

/// How long an action waits for the user to allow it.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The guard wants the user to allow something first.
pub struct GuardAsk {
    /// Who wants it: an agent's name, or an automation's.
    pub who: String,
    /// Where it runs.
    pub host: String,
    pub script: String,
    /// Why the guard asks.
    pub reason: String,
    /// An Ask me step's own question, asked in place of "Allow this command?".
    pub question: Option<String>,
    /// True when the user allows it. Dropped when there is no one to ask.
    pub answer: oneshot::Sender<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Run,
    Allowed,
    Blocked,
    Denied,
    /// Asked, and no answer came in time.
    NoAnswer,
    /// Nobody to ask: the app is closed.
    NoOne,
}

pub struct Checked {
    pub verdict: Verdict,
    pub outcome: Outcome,
}

impl Checked {
    pub fn runs(&self) -> bool {
        matches!(self.outcome, Outcome::Run | Outcome::Allowed)
    }

    /// What the user said, when asked.
    pub fn answer(&self) -> Option<bool> {
        match self.outcome {
            Outcome::Allowed => Some(true),
            Outcome::Denied => Some(false),
            _ => None,
        }
    }
}

/// Checks `action` with the guard and, when it says to ask, asks the user
/// through `ask`. `ask` gives false when there is no one to send it to.
pub async fn check(services: &Services, action: Action, host: &str, ask: impl FnOnce(GuardAsk) -> bool) -> Checked {
    let script = action.detail.clone();
    let who = action.requested_by.clone();
    let verdict = guard(services, action).await;
    let outcome = match verdict.decision {
        Decision::Run => Outcome::Run,
        Decision::Block => Outcome::Blocked,
        Decision::Ask => {
            let (answer, answered) = oneshot::channel();
            let sent = ask(GuardAsk { who, host: host.to_owned(), script, reason: verdict.reason.clone(), question: None, answer });
            if sent {
                match tokio::time::timeout(ASK_TIMEOUT, answered).await {
                    Ok(Ok(true)) => Outcome::Allowed,
                    Ok(Ok(false)) => Outcome::Denied,
                    // Dropped without an answer: no window to ask in.
                    Ok(Err(_)) => Outcome::NoOne,
                    Err(_) => Outcome::NoAnswer,
                }
            } else {
                Outcome::NoOne
            }
        }
    };
    Checked { verdict, outcome }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_use_can_have_its_own_model() {
        let mut settings = Settings::default();
        settings.ai.llm_model = "big".into();
        settings.ai.write_model = " small ".into();
        assert_eq!(model_for(&settings, Feature::Write), "small");
        assert_eq!(model_for(&settings, Feature::Decide), "big");
        assert_eq!(model_for(&settings, Feature::Guard), "big");
    }

    #[test]
    fn the_master_switch_turns_every_feature_off() {
        let mut settings = Settings::default();
        for feature in [Feature::Guard, Feature::Decide, Feature::Write, Feature::Builder, Feature::Agent, Feature::Computer] {
            assert!(!allowed(&settings, feature), "{feature:?} runs with AI off");
        }
        settings.ai.on = true;
        assert!(allowed(&settings, Feature::Guard));
        // Computer control stays off until turned on by itself.
        assert!(!allowed(&settings, Feature::Computer));
        settings.ai.computer = true;
        assert!(allowed(&settings, Feature::Computer));
        settings.ai.agent = false;
        assert!(!allowed(&settings, Feature::Computer));
    }
}
