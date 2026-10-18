//! The open source work Somme is built from, for Settings › Credits. The
//! list is `credits.json`, made from `cargo metadata`; a test fails when it
//! no longer matches `Cargo.lock`.

use std::sync::LazyLock;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window, div,
    px, uniform_list,
};
use serde::{Deserialize, Serialize};

use crate::ui::common::Lucide;
use crate::ui::filter::fuzzy_score;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credits {
    /// What ships in the app.
    libraries: Vec<Library>,
    /// What only builds or tests it, or comes with features it leaves off,
    /// as `name version`, so the test can tell a new library from one that
    /// never ships.
    not_shipped: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    pub versions: Vec<String>,
    pub license: Option<String>,
    pub link: String,
}

/// Icons, logos and speech models, which are not Rust libraries.
pub const ARTWORK: &[(&str, &str, &str)] = &[
    ("Lucide icons", "ISC", "https://lucide.dev"),
    ("Simple Icons, for system logos", "CC0-1.0", "https://simpleicons.org"),
    ("Whisper speech model, by OpenAI", "MIT", "https://github.com/openai/whisper"),
    ("Silero VAD, which finds speech", "MIT", "https://github.com/snakers4/silero-vad"),
];

static CREDITS: LazyLock<Credits> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../credits.json")).expect("credits.json is valid"));

pub fn libraries() -> &'static [Library] {
    &CREDITS.libraries
}

/// A filterable list of every library, drawn only where it scrolls into view.
pub struct CreditsList {
    filter: Entity<InputState>,
    /// Indexes into [`libraries`] that match the filter.
    shown: Vec<usize>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl CreditsList {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter libraries"));
        let subscription = cx.subscribe(&filter, |list, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                list.refilter(cx);
            }
        });
        Self {
            filter,
            shown: (0..libraries().len()).collect(),
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().trim().to_owned();
        self.shown = libraries()
            .iter()
            .enumerate()
            .filter(|(_, library)| {
                let text = format!("{} {}", library.name, library.license.as_deref().unwrap_or_default());
                fuzzy_score(&query, &text).is_some()
            })
            .map(|(ix, _)| ix)
            .collect();
        cx.notify();
    }

    fn render_row(&self, ix: usize, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let library = &libraries()[ix];
        let link = library.link.clone();
        h_flex()
            .id(("credit", ix))
            .w_full()
            .h(px(30.))
            .px_2()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|row| row.bg(theme.accent))
            .child(div().text_sm().truncate().child(library.name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(library.versions.join(", ")),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(240.))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(library.license.clone().unwrap_or_else(|| "See its page".to_owned()))),
            )
            .on_click(move |_, _, cx| cx.open_url(&link))
    }
}

impl Render for CreditsList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = if self.shown.len() == libraries().len() {
            format!("{} libraries", libraries().len())
        } else {
            format!("{} of {} libraries", self.shown.len(), libraries().len())
        };
        let list = if self.shown.is_empty() {
            div().p_3().text_sm().text_color(theme.muted_foreground).child("No library matches.").into_any_element()
        } else {
            uniform_list(
                "credits",
                self.shown.len(),
                cx.processor(|list, range: std::ops::Range<usize>, _, cx| {
                    range.map(|row| list.render_row(list.shown[row], cx)).collect::<Vec<_>>()
                }),
            )
            .size_full()
            .track_scroll(&self.scroll)
            .into_any_element()
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .child(div().flex_1().child(Input::new(&self.filter).prefix(Icon::new(Lucide::Search).size(px(14.)))))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(count)),
            )
            .child(
                div()
                    .h(px(420.))
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::process::Command;

    use serde_json::Value;

    use super::*;

    const ROOT: &str = "somme-desktop";
    const HINT: &str = "run `cargo test -p somme-desktop write_credits -- --ignored`";

    fn workspace() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    /// Package ids reachable from the app through dependencies of `kinds`
    /// (None is a normal dependency).
    fn reachable(meta: &Value, root: &str, kinds: &[Option<&str>]) -> BTreeSet<String> {
        let nodes: BTreeMap<&str, &Value> =
            meta["resolve"]["nodes"].as_array().unwrap().iter().map(|node| (node["id"].as_str().unwrap(), node)).collect();
        let mut seen = BTreeSet::new();
        let mut stack = vec![root.to_owned()];
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for dep in nodes[id.as_str()]["deps"].as_array().unwrap() {
                let wanted = dep["dep_kinds"].as_array().unwrap().iter().any(|kind| kinds.contains(&kind["kind"].as_str()));
                if wanted {
                    stack.push(dep["pkg"].as_str().unwrap().to_owned());
                }
            }
        }
        seen
    }

    fn metadata(all_features: bool) -> Value {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let mut command = Command::new(cargo);
        command.args(["metadata", "--format-version", "1", "--locked", "--manifest-path"]);
        command.arg(workspace().join("Cargo.toml"));
        if all_features {
            command.arg("--all-features");
        }
        let out = command.output().unwrap();
        assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn collect() -> Credits {
        let meta = metadata(false);
        // Cargo.lock holds what every feature could pull in.
        let every_feature = metadata(true);
        let packages: BTreeMap<&str, &Value> =
            every_feature["packages"].as_array().unwrap().iter().map(|package| (package["id"].as_str().unwrap(), package)).collect();
        let root = packages.values().find(|package| package["name"] == ROOT).unwrap()["id"].as_str().unwrap();
        // Our own crates have no source.
        let from_registry = |ids: BTreeSet<String>| -> Vec<&Value> {
            ids.iter().map(|id| packages[id.as_str()]).filter(|package| !package["source"].is_null()).collect()
        };
        let shipped = reachable(&meta, root, &[None]);
        let everything = reachable(&every_feature, root, &[None, Some("build"), Some("dev")]);
        let not_shipped = everything.difference(&shipped).cloned().collect();

        let mut libraries: BTreeMap<String, Library> = BTreeMap::new();
        let mut shipped = from_registry(shipped);
        shipped.sort_by_key(|package| {
            let version = package["version"].as_str().unwrap();
            let numbers: Vec<u64> = version.split(['.', '-', '+']).map(|part| part.parse().unwrap_or(0)).collect();
            (package["name"].as_str().unwrap().to_lowercase(), numbers)
        });
        for package in shipped {
            let text = |key: &str| package[key].as_str().filter(|text| !text.is_empty()).map(str::to_owned);
            let name = text("name").unwrap();
            let link = text("repository")
                .or_else(|| text("homepage"))
                .unwrap_or_else(|| format!("https://crates.io/crates/{name}"));
            let library = libraries.entry(name.to_lowercase()).or_insert_with(|| Library {
                name: name.clone(),
                versions: Vec::new(),
                license: None,
                link: String::new(),
            });
            library.versions.push(text("version").unwrap());
            // The newest version's details win.
            library.license = text("license").or(library.license.take());
            library.link = link;
        }
        let mut not_shipped: Vec<String> = from_registry(not_shipped)
            .iter()
            .map(|package| format!("{} {}", package["name"].as_str().unwrap(), package["version"].as_str().unwrap()))
            .collect();
        not_shipped.sort();
        Credits {
            libraries: libraries.into_values().collect(),
            not_shipped,
        }
    }

    /// Every `name version` from crates.io that `Cargo.lock` reaches from the app.
    fn locked() -> BTreeSet<String> {
        let lock = std::fs::read_to_string(workspace().join("Cargo.lock")).unwrap();
        struct Locked {
            name: String,
            version: String,
            registry: bool,
            deps: Vec<String>,
        }
        let mut packages: Vec<Locked> = Vec::new();
        let mut in_deps = false;
        for line in lock.lines() {
            let unquote = |text: &str| text.trim().trim_end_matches(',').trim_matches('"').to_owned();
            if line == "[[package]]" {
                packages.push(Locked {
                    name: String::new(),
                    version: String::new(),
                    registry: false,
                    deps: Vec::new(),
                });
                in_deps = false;
            } else if let Some(package) = packages.last_mut() {
                if in_deps {
                    if line.starts_with(']') {
                        in_deps = false;
                    } else {
                        package.deps.push(unquote(line));
                    }
                } else if let Some(name) = line.strip_prefix("name = ") {
                    package.name = unquote(name);
                } else if let Some(version) = line.strip_prefix("version = ") {
                    package.version = unquote(version);
                } else if line.starts_with("source = ") {
                    package.registry = true;
                } else if line.starts_with("dependencies = [") {
                    in_deps = true;
                }
            }
        }
        let find = |dep: &str| -> usize {
            let mut words = dep.split(' ');
            let name = words.next().unwrap();
            let version = words.next();
            packages
                .iter()
                .position(|package| package.name == name && version.is_none_or(|version| package.version == version))
                .unwrap_or_else(|| panic!("{dep} is not in Cargo.lock"))
        };
        let mut seen = BTreeSet::new();
        let mut stack = vec![find(ROOT)];
        while let Some(ix) = stack.pop() {
            if seen.insert(ix) {
                stack.extend(packages[ix].deps.iter().map(|dep| find(dep)));
            }
        }
        seen.into_iter()
            .map(|ix| &packages[ix])
            .filter(|package| package.registry)
            .map(|package| format!("{} {}", package.name, package.version))
            .collect()
    }

    #[test]
    fn credits_match_the_lock_file() {
        let mut credited: BTreeSet<String> = CREDITS.not_shipped.iter().cloned().collect();
        for library in &CREDITS.libraries {
            credited.extend(library.versions.iter().map(|version| format!("{} {version}", library.name)));
        }
        let locked = locked();
        let missing: Vec<_> = locked.difference(&credited).take(5).collect();
        let gone: Vec<_> = credited.difference(&locked).take(5).collect();
        assert!(missing.is_empty() && gone.is_empty(), "credits.json is out of date ({HINT}): new {missing:?}, removed {gone:?}");
        assert!(CREDITS.libraries.iter().all(|library| library.link.starts_with("http")));
    }

    #[test]
    #[ignore = "rewrites credits.json from cargo metadata"]
    fn write_credits() {
        let mut json = serde_json::to_string_pretty(&collect()).unwrap();
        json.push('\n');
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/credits.json"), json).unwrap();
    }
}
