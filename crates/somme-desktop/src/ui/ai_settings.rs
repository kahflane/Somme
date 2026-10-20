//! Settings → AI: one switch for all of it, a switch for each feature, and
//! the user's own LLM and Jev keys. Somme works fully with AI off.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectState};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use somme_ai::jev::Jev;
use somme_ai::llm::{self, Llm, LlmConfig, PRESETS, Region};

use crate::ai::{self, KeyName};
use crate::runtime;
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::common::{APP_NAME, Lucide, ok_color};
use crate::ui::fields::Choice;
use crate::ui::settings_page::switch;

/// A provider from [`PRESETS`], by its place there; None for any other server.
type Provider = Option<usize>;
type ProviderState = Entity<SelectState<SearchableVec<SelectGroup<Choice<Provider>>>>>;
type ModelState = Entity<SelectState<SearchableVec<String>>>;

pub fn ai_page(cx: &App) -> SettingPage {
    // Everything under the main switch waits for it.
    let off = !Settings::get(cx).ai.on;
    SettingPage::new("AI")
        .icon(Icon::new(Lucide::Sparkles))
        .group(
            SettingGroup::new()
                .title("AI")
                .description(format!(
                    "{APP_NAME} works fully without AI. When it is on, it uses your own LLM and Jev keys below. Nothing goes to an AI service while it is off."
                ))
                .item(
                    SettingItem::new("Use AI", switch(|s| s.ai.on, |s, v| s.ai.on = v))
                        .description("Turns every AI feature on or off at once."),
                )
                .item(
                    SettingItem::new("Guard checks", switch(|s| s.ai.guard, |s, v| s.ai.guard = v))
                        .description(
                            "Jev or the LLM checks agents' commands against your guard rules. When off, only the quick check on this computer runs.",
                        )
                        .disabled(off),
                ),
        )
        .group(
            SettingGroup::new()
                .title("LLM")
                .description("Pick a provider, or any server that speaks the OpenAI API. A server on this computer, like Ollama, keeps everything here.")
                .item(
                    SettingItem::render(|_, window, cx| {
                        div().w_full().child(window.use_keyed_state("ai-llm", cx, |window, cx| KeySetup::new(KeyName::Llm, window, cx)))
                    })
                    .disabled(off),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Jev")
                .description("Jev answers yes or no questions with how sure it is. The guard asks Jev first, then the LLM.")
                .item(
                    SettingItem::render(|_, window, cx| {
                        div().w_full().child(window.use_keyed_state("ai-jev", cx, |window, cx| KeySetup::new(KeyName::Jev, window, cx)))
                    })
                    .disabled(off),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Automations")
                .description("AI steps and building by chatting. Each uses the model above unless you pick another here.")
                .item(
                    SettingItem::new("Build by chatting", switch(|s| s.ai.builder, |s, v| s.ai.builder = v))
                        .description("Describe an automation and the LLM puts its steps on the canvas. Nothing runs until you press Run.")
                        .disabled(off),
                )
                .item(
                    SettingItem::new("Let the builder look around", switch(|s| s.ai.builder_looks, |s, v| s.ai.builder_looks = v))
                        .description("While building, it may run commands that only look, like listing folders and their sizes, on this computer and your servers, so its steps fit the real machine. What they show goes to the LLM with secrets taken out.")
                        .disabled(off),
                )
                .item(model_item("Model for building", |s| &s.ai.builder_model, |s, v| s.ai.builder_model = v).disabled(off))
                .item(SettingItem::render(|_, _, cx| remembered(cx)).disabled(off))
                .item(SettingItem::render(|_, _, cx| skills(cx)).disabled(off))
                .item(
                    SettingItem::new("Decide steps", switch(|s| s.ai.decide, |s, v| s.ai.decide = v))
                        .description("Yes or no questions about a step's output. Jev answers first, then the LLM.")
                        .disabled(off),
                )
                .item(model_item("Model for Decide", |s| &s.ai.decide_model, |s, v| s.ai.decide_model = v).disabled(off))
                .item(
                    SettingItem::new("Write steps", switch(|s| s.ai.write, |s, v| s.ai.write = v))
                        .description("The LLM writes text, like a short summary for a notification.")
                        .disabled(off),
                )
                .item(model_item("Model for Write", |s| &s.ai.write_model, |s, v| s.ai.write_model = v).disabled(off))
                .item(
                    SettingItem::new("Agent steps", switch(|s| s.ai.agent, |s, v| s.ai.agent = v))
                        .description(
                            "An LLM with a goal runs commands, reads and changes files on the places you pick. The guard checks each thing it does. Use a model that calls tools.",
                        )
                        .disabled(off),
                )
                .item(model_item("Model for Agent", |s| &s.ai.agent_model, |s, v| s.ai.agent_model = v).disabled(off))
                .item(
                    SettingItem::new("Agents may use the screen", switch(|s| s.ai.computer, |s, v| s.ai.computer = v))
                        .description(
                            "Agent steps you allow see this computer's screen, click and type, in the apps you name only. Screenshots go to your LLM and are not kept. A bar at the top shows when it happens; press Esc to stop. Use a model that reads images; small local models often miss what they click.",
                        )
                        .disabled(off),
                )
                .item(SettingItem::render(|_, _, cx| screen_access(cx)).disabled(off))
                .item(
                    SettingItem::new(
                        "AI calls per run",
                        SettingField::number_input(
                            NumberFieldOptions { min: 1., max: 500., step: 1. },
                            |cx| Settings::get(cx).ai.run_ai_calls as f64,
                            |calls, cx| Settings::update(cx, |s| s.ai.run_ai_calls = calls.clamp(1., 500.) as u32),
                        )
                        .default_value(Settings::default().ai.run_ai_calls as f64),
                    )
                    .description("A run stops asking AI after this many calls.")
                    .disabled(off),
                ),
        )
}

/// Whether agents can use this computer's screen, and on macOS a button to
/// ask the system for access.
/// What the builder learned from this user, and a way to forget it.
fn remembered(cx: &App) -> gpui_kit::Div {
    let data_dir = Store::services(cx).data_dir.clone();
    let facts = crate::automations::memory::load(&data_dir);
    let muted = cx.theme().muted_foreground;
    let said = if facts.is_empty() {
        "The assistant keeps short facts it learns about you and your machines, like where your projects are. Nothing yet.".to_owned()
    } else {
        format!("What the assistant remembers about you and your machines ({}). Each chat starts with these.", facts.len())
    };
    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_sm().text_color(muted).whitespace_normal().child(said))
        .children(facts.into_iter().rev().enumerate().map(|(ix, fact)| {
            let (data_dir, text) = (data_dir.clone(), fact.text.clone());
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().whitespace_normal().text_sm().child(fact.text))
                .child(Button::new(("forget-fact", ix)).ghost().xsmall().icon(Icon::new(Lucide::X)).tooltip("Forget this").on_click(move |_, window, _| {
                    crate::automations::memory::forget(&data_dir, &text);
                    window.refresh();
                }))
        }))
}

fn skills(cx: &App) -> gpui_kit::Div {
    let data_dir = Store::services(cx).data_dir.clone();
    let own = crate::automations::skills::own(&data_dir).len();
    let built_in = crate::automations::skills::built_in().len();
    let yours = match own {
        0 => "none of yours yet".to_owned(),
        1 => "1 of yours".to_owned(),
        n => format!("{n} of yours"),
    };
    let said = format!(
        "Skills are know-how the builder reads for kinds of jobs: {built_in} come with {APP_NAME}, {yours}. To add one, put a folder with a SKILL.md in the skills folder. Hermes Agent and Claude skills use the same format."
    );
    let folder = crate::automations::skills::folder(&data_dir);
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().flex_1().min_w_0().whitespace_normal().text_sm().text_color(cx.theme().muted_foreground).child(said))
        .child(Button::new("open-skills").small().label("Open folder").on_click(move |_, _, cx| {
            match std::fs::create_dir_all(&folder) {
                Ok(()) => cx.open_with_system(&folder),
                Err(err) => tracing::warn!(%err, "could not make the skills folder"),
            }
        }))
}

fn screen_access(cx: &App) -> gpui_kit::Div {
    let off = !Settings::get(cx).ai.on;
    let ready = crate::computer::platform().and_then(|platform| platform.ready());
    let asks = ready.is_err() && cfg!(target_os = "macos");
    let said = match ready {
        Ok(()) => format!("{APP_NAME} can see the screen, click and type here."),
        Err(err) => err,
    };
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().flex_1().text_sm().text_color(cx.theme().muted_foreground).child(said))
        .when(asks, |row| {
            row.child(Button::new("screen-access").small().label("Ask macOS").disabled(off).on_click(|_, _, _| {
                if let Ok(platform) = crate::computer::platform() {
                    platform.ask_for_access();
                }
            }))
        })
}

/// A model name for one use. Empty: the LLM's model.
fn model_item(title: &'static str, get: fn(&Settings) -> &String, set: fn(&mut Settings, String)) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::input(
            move |cx| SharedString::from(get(Settings::get(cx)).clone()),
            move |value, cx| Settings::update(cx, |s| set(s, value.trim().to_owned())),
        ),
    )
    .description("Empty: the model above.")
}

/// The fields for one service: the LLM or Jev. For the LLM: pick a
/// provider, paste the key, then pick one of the models it lists.
pub struct KeySetup {
    which: KeyName,
    /// Only the LLM has these.
    provider: Option<ProviderState>,
    base_url: Option<Entity<InputState>>,
    model: Entity<InputState>,
    /// The models the server listed, to pick from.
    model_list: Option<ModelState>,
    /// The user types the model instead of picking it.
    typing_model: bool,
    key: Entity<InputState>,
    key_saved: bool,
    busy: bool,
    /// The last result: whether it went well, and what happened.
    status: Option<(bool, String)>,
}

/// The providers, grouped by where their servers are.
fn provider_groups() -> Vec<SelectGroup<Choice<Provider>>> {
    let group = |title: &'static str, region: Region| {
        SelectGroup::new(title).items(
            PRESETS
                .iter()
                .enumerate()
                .filter(|(_, preset)| preset.region == region)
                .map(|(ix, preset)| Choice { value: Some(ix), label: preset.name.into() }),
        )
    };
    vec![
        group("Popular", Region::Global),
        group("China", Region::China),
        group("On this computer", Region::Local),
        SelectGroup::new("Other").item(Choice { value: None, label: "Another OpenAI-compatible server".into() }),
    ]
}

fn provider_of(base_url: &str) -> Provider {
    let preset = llm::preset_for(base_url)?;
    PRESETS.iter().position(|known| known == preset)
}

impl KeySetup {
    fn new(which: KeyName, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ai = Settings::get(cx).ai.clone();
        let (provider, base_url) = if which == KeyName::Llm {
            let base_url =
                cx.new(|cx| InputState::new(window, cx).placeholder("https://api.openai.com/v1").default_value(ai.llm_base_url.clone()));
            let picked = provider_of(&ai.llm_base_url);
            let provider = cx.new(|cx| {
                let mut state = SelectState::new(SearchableVec::new(provider_groups()), None, window, cx).searchable(true);
                if !ai.llm_base_url.trim().is_empty() {
                    state.set_selected_value(&picked, window, cx);
                }
                state
            });
            cx.subscribe_in(&provider, window, |this, _, event: &SelectEvent<_>, window, cx| {
                if let SelectEvent::Confirm(Some(picked)) = event {
                    this.pick_provider(*picked, window, cx);
                }
            })
            .detach();
            // The provider follows the address, and the note under it says where text goes.
            cx.subscribe_in(&base_url, window, |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let picked = provider_of(&input.read(cx).value());
                    if let Some(provider) = &this.provider {
                        provider.update(cx, |state, cx| state.set_selected_value(&picked, window, cx));
                    }
                    cx.notify();
                }
            })
            .detach();
            (Some(provider), Some(base_url))
        } else {
            (None, None)
        };
        let (model_hint, model) = match which {
            KeyName::Llm => ("Type the model's name, like gpt-4o-mini", ai.llm_model),
            KeyName::Jev => ("Model", ai.jev_model),
        };
        let model = cx.new(|cx| InputState::new(window, cx).placeholder(model_hint).default_value(model));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Paste your API key").masked(true));
        let services = Store::services(cx);
        cx.spawn(async move |this, cx| {
            let saved = runtime::io(async move { ai::has_key(&services, which).await }).await;
            let _ = this.update(cx, |this, cx| {
                this.key_saved = saved;
                cx.notify();
            });
        })
        .detach();
        Self {
            which,
            provider,
            base_url,
            model,
            model_list: None,
            typing_model: false,
            key,
            key_saved: false,
            busy: false,
            status: None,
        }
    }

    fn text(input: &Entity<InputState>, cx: &Context<Self>) -> String {
        input.read(cx).value().trim().to_owned()
    }

    /// Fills in the provider's address. A server on this computer needs no
    /// key, so its models are listed at once.
    fn pick_provider(&mut self, picked: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let Some(base_url) = self.base_url.clone() else { return };
        let preset = picked.and_then(|ix| PRESETS.get(ix));
        base_url.update(cx, |input, cx| input.set_value(preset.map_or("", |preset| preset.base_url), window, cx));
        self.model_list = None;
        self.typing_model = false;
        self.status = None;
        match preset {
            Some(preset) if !preset.needs_key => self.run(Then::Models, window, cx),
            Some(preset) => self.status = Some((true, format!("Paste your {} key, then press Save to see its models.", preset.name))),
            None => self.status = Some((true, "Type the server's address, up to the version, like https://example.com/v1.".into())),
        }
        cx.notify();
    }

    /// Uses a model picked from the list.
    fn pick_model(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |input, cx| input.set_value(name.clone(), window, cx));
        self.save_fields(cx);
        self.status = Some((true, format!("Using {name}.")));
        cx.notify();
    }

    /// Saves the address and model to the settings.
    fn save_fields(&self, cx: &mut Context<Self>) {
        let model = Self::text(&self.model, cx);
        let base_url = self.base_url.as_ref().map(|input| Self::text(input, cx).trim_end_matches('/').to_owned());
        let which = self.which;
        Settings::update(cx, |s| match which {
            KeyName::Llm => {
                s.ai.llm_base_url = base_url.unwrap_or_default();
                s.ai.llm_model = model;
            }
            KeyName::Jev => {
                s.ai.jev_model = if model.is_empty() { somme_ai::jev::DEFAULT_MODEL.to_owned() } else { model };
            }
        });
    }

    /// Saves everything, then does `then` with the service: test it, or
    /// list its models. Saving the LLM's key lists its models too.
    fn run(&mut self, then: Then, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.save_fields(cx);
        let typed = Self::text(&self.key, cx);
        let (services, settings, which) = (Store::services(cx), Settings::get(cx).clone(), self.which);
        self.busy = true;
        self.status = None;
        cx.notify();
        let saving_key = !typed.is_empty() || then == Then::RemoveKey;
        let then = if then == Then::Save && which == KeyName::Llm && !settings.ai.llm_base_url.is_empty() { Then::Models } else { then };
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::io(async move {
                if saving_key {
                    ai::save_key(&services, which, &typed).await.map_err(|err| format!("Could not save the key: {err:#}"))?;
                }
                let keys = ai::keys(&services).await;
                match (then, which) {
                    (Then::Save, _) => Ok(Done::Said("Saved.".into())),
                    (Then::RemoveKey, _) => Ok(Done::Said("The key is removed.".into())),
                    (Then::Test, KeyName::Llm) => {
                        let config = ai::llm_config(&settings, &keys).ok_or("Pick a model first.")?;
                        let answer = Llm::new(&config).test().await.map_err(|err| err.to_string())?;
                        Ok(Done::Said(format!("It works. The model said \u{201c}{answer}\u{201d}.")))
                    }
                    (Then::Test, KeyName::Jev) => {
                        let key = keys.jev.ok_or("Paste your Jev key first.")?;
                        Jev::new(key, settings.ai.jev_model.clone()).test().await.map_err(|err| err.to_string())?;
                        Ok(Done::Said("It works.".into()))
                    }
                    (Then::Models, _) => {
                        let config = LlmConfig {
                            base_url: settings.ai.llm_base_url.clone(),
                            api_key: keys.llm,
                            model: settings.ai.llm_model.clone(),
                        };
                        if config.base_url.is_empty() {
                            return Err("Pick a provider or type the address first.".into());
                        }
                        // A failed list still leaves the model to type.
                        Ok(Llm::new(&config).models().await.map_or_else(|err| Done::NoModels(err.to_string()), Done::Models))
                    }
                }
            })
            .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if saving_key && result.is_ok() {
                    this.key_saved = then != Then::RemoveKey;
                    this.key.update(cx, |input, cx| input.set_value("", window, cx));
                    ai::check_saved_keys(Store::services(cx), cx);
                }
                this.status = Some(match result {
                    Ok(Done::Said(text)) => (true, text),
                    Ok(Done::Models(models)) if !models.is_empty() => this.show_models(models, window, cx),
                    Ok(Done::Models(_)) => this.no_models("The server listed no models.".into()),
                    Ok(Done::NoModels(err)) => this.no_models(format!("Could not list models: {err}.")),
                    Err(err) => (false, err),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts the listed models in the model dropdown.
    fn show_models(&mut self, models: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> (bool, String) {
        let count = models.len();
        let current = Self::text(&self.model, cx);
        let known = models.contains(&current);
        let list = cx.new(|cx| {
            let mut state = SelectState::new(SearchableVec::new(models), None, window, cx).searchable(true);
            if known {
                state.set_selected_value(&current, window, cx);
            }
            state
        });
        cx.subscribe_in(&list, window, |this, _, event: &SelectEvent<_>, window, cx| {
            if let SelectEvent::Confirm(Some(name)) = event {
                this.pick_model(name.clone(), window, cx);
            }
        })
        .detach();
        self.model_list = Some(list);
        self.typing_model = !current.is_empty() && !known;
        let said = if count == 1 { "Found 1 model.".to_owned() } else { format!("Found {count} models.") };
        (true, if known { format!("{said} Using {current}.") } else { format!("{said} Pick one.") })
    }

    fn no_models(&mut self, why: String) -> (bool, String) {
        self.model_list = None;
        self.typing_model = true;
        (false, format!("{why} Type the model's name instead."))
    }

    fn where_text_goes(&self, cx: &Context<Self>) -> Option<String> {
        let address = Self::text(self.base_url.as_ref()?, cx);
        if address.is_empty() {
            return None;
        }
        if llm::is_local(&address) {
            return Some("This server is on this computer, so nothing leaves it.".into());
        }
        let host = address.split_once("://").map_or(address.as_str(), |(_, rest)| rest).split('/').next().unwrap_or_default();
        Some(format!("Commands and text go to {host}. {APP_NAME} hides the passwords and keys it knows first."))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Then {
    Save,
    Test,
    Models,
    RemoveKey,
}

enum Done {
    Said(String),
    Models(Vec<String>),
    /// Listing failed, with why.
    NoModels(String),
}

fn field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    v_flex().gap_1().child(div().text_sm().child(label)).child(input)
}

impl Render for KeySetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let off = !Settings::get(cx).ai.on;
        let id = match self.which {
            KeyName::Llm => "llm",
            KeyName::Jev => "jev",
        };
        let hint = |text: String| div().text_xs().text_color(theme.muted_foreground).child(SharedString::from(text));
        let local = self.base_url.as_ref().is_some_and(|input| llm::is_local(&Self::text(input, cx)));
        let key_hint = if self.key_saved {
            "A key is saved, locked with your master password. Paste a new one to replace it."
        } else if local {
            "A server on this computer needs no key."
        } else {
            "No key is saved yet."
        };
        let busy = self.busy;
        let model = match &self.model_list {
            Some(list) if !self.typing_model => v_flex()
                .gap_1()
                .child(Select::new(list).placeholder("Pick a model").search_placeholder("Find a model").disabled(off || busy))
                .child(
                    h_flex().child(
                        Button::new("llm-type-model")
                            .xsmall()
                            .ghost()
                            .label("Type a name instead")
                            .disabled(off)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.typing_model = true;
                                cx.notify();
                            })),
                    ),
                ),
            list => v_flex().gap_1().child(Input::new(&self.model).disabled(off)).when(list.is_some(), |col| {
                col.child(
                    h_flex().child(
                        Button::new("llm-pick-model")
                            .xsmall()
                            .ghost()
                            .label("Pick from the list")
                            .disabled(off)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.typing_model = false;
                                cx.notify();
                            })),
                    ),
                )
            }),
        };
        let buttons = h_flex()
            .gap_2()
            .child(
                Button::new(format!("{id}-save"))
                    .small()
                    .primary()
                    .label("Save")
                    .disabled(off || busy)
                    .on_click(cx.listener(|this, _, window, cx| this.run(Then::Save, window, cx))),
            )
            .when(self.which == KeyName::Llm, |row| {
                row.child(
                    Button::new("llm-models")
                        .small()
                        .label("Find models")
                        .disabled(off || busy)
                        .on_click(cx.listener(|this, _, window, cx| this.run(Then::Models, window, cx))),
                )
            })
            .child(
                Button::new(format!("{id}-test"))
                    .small()
                    .label("Test")
                    .loading(busy)
                    .disabled(off || busy)
                    .on_click(cx.listener(|this, _, window, cx| this.run(Then::Test, window, cx))),
            )
            .when(self.key_saved, |row| {
                row.child(
                    Button::new(format!("{id}-remove-key"))
                        .small()
                        .ghost()
                        .icon(Icon::new(Lucide::Trash))
                        .label("Remove key")
                        .disabled(off || busy)
                        .on_click(cx.listener(|this, _, window, cx| this.run(Then::RemoveKey, window, cx))),
                )
            });
        let status = self.status.clone().map(|(ok, text)| {
            div().text_sm().text_color(if ok { ok_color(cx) } else { theme.danger }).child(SharedString::from(text))
        });
        v_flex()
            .w_full()
            .gap_3()
            .when_some(self.provider.clone(), |col, provider| {
                col.child(field("Provider", Select::new(&provider).placeholder("Pick a provider").search_placeholder("Find a provider").disabled(off)))
            })
            .when_some(self.base_url.clone(), |col, base_url| {
                col.child(field("Address", Input::new(&base_url).disabled(off))).children(self.where_text_goes(cx).map(&hint))
            })
            .child(field("API key", Input::new(&self.key).mask_toggle().disabled(off)))
            .child(hint(key_hint.into()))
            .child(field("Model", model))
            .child(buttons)
            .children(status)
    }
}
