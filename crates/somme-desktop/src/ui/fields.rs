use gpui_kit::component::select::{SearchableVec, SelectItem, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::{App, AppContext as _, Entity, SharedString, Window};
use somme_application::dto::{HostInput, HostView, SecretUpdate};
use somme_domain::Id;

use crate::store::Store;

/// One row of a dropdown: a value and the text shown for it.
#[derive(Clone)]
pub struct Choice<V> {
    pub value: V,
    pub label: SharedString,
}

impl<V: Clone + PartialEq> SelectItem for Choice<V> {
    type Value = V;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &V {
        &self.value
    }
}

pub type ChoiceState<V> = Entity<SelectState<SearchableVec<Choice<V>>>>;

pub fn choice_select<V: Clone + PartialEq + 'static>(
    items: Vec<Choice<V>>,
    selected: &V,
    window: &mut Window,
    cx: &mut App,
) -> ChoiceState<V> {
    let index = items.iter().position(|item| item.value == *selected).map(IndexPath::new);
    let searchable = items.len() > 6;
    cx.new(|cx| SelectState::new(SearchableVec::new(items), index, window, cx).searchable(searchable))
}

pub fn selected<V: Clone + PartialEq + 'static>(state: &ChoiceState<V>, cx: &App) -> Option<V> {
    state.read(cx).selected_value().cloned()
}

fn none_choice(label: &str) -> Choice<Option<Id>> {
    Choice {
        value: None,
        label: label.to_owned().into(),
    }
}

pub fn group_choices(cx: &App) -> Vec<Choice<Option<Id>>> {
    let store = Store::global(cx);
    let store = store.read(cx);
    let mut items = vec![none_choice("No group")];
    let mut groups: Vec<_> = store.groups.iter().map(|g| (group_path(g.id, cx), g.id)).collect();
    groups.sort_by_key(|(path, _)| path.to_lowercase());
    items.extend(groups.into_iter().map(|(path, id)| Choice {
        value: Some(id),
        label: path.into(),
    }));
    items
}

/// "Parent / Child" names for nested groups.
pub fn group_path(id: Id, cx: &App) -> String {
    let store = Store::global(cx);
    let store = store.read(cx);
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(id) = current {
        let Some(group) = store.group(id) else { break };
        parts.push(group.name.clone());
        current = group.parent_id;
        if parts.len() > 16 {
            break;
        }
    }
    parts.reverse();
    parts.join(" / ")
}

pub fn key_choices(cx: &App) -> Vec<Choice<Option<Id>>> {
    let store = Store::global(cx);
    let mut items = vec![none_choice("No key")];
    items.extend(store.read(cx).keys.iter().map(|k| Choice {
        value: Some(k.id),
        label: format!("{} ({})", k.label, k.algorithm).into(),
    }));
    items
}

pub fn identity_choices(cx: &App) -> Vec<Choice<Option<Id>>> {
    let store = Store::global(cx);
    let mut items = vec![none_choice("No identity")];
    items.extend(store.read(cx).identities.iter().map(|i| Choice {
        value: Some(i.id),
        label: format!("{} ({})", i.label, i.username).into(),
    }));
    items
}

pub fn host_choices(except: Option<Id>, cx: &App) -> Vec<Choice<Option<Id>>> {
    let store = Store::global(cx);
    let mut items = vec![none_choice("Direct connection")];
    items.extend(
        store
            .read(cx)
            .hosts
            .iter()
            .filter(|h| Some(h.id) != except)
            .map(|h| Choice {
                value: Some(h.id),
                label: h.label.clone().into(),
            }),
    );
    items
}

pub fn snippet_choices(cx: &App) -> Vec<Choice<Option<Id>>> {
    let store = Store::global(cx);
    let mut items = vec![none_choice("None")];
    items.extend(store.read(cx).snippets.iter().map(|s| Choice {
        value: Some(s.id),
        label: s.label.clone().into(),
    }));
    items
}

/// A save request that keeps everything about the host as it is.
pub fn host_input(host: &HostView) -> HostInput {
    HostInput {
        id: Some(host.id),
        label: host.label.clone(),
        address: host.address.clone(),
        port: host.port,
        group_id: host.group_id,
        identity_id: host.identity_id,
        username: host.username.clone(),
        password: SecretUpdate::Keep,
        key_id: host.key_id,
        jump_host_id: host.jump_host_id,
        startup_snippet_id: host.startup_snippet_id,
        tags: host.tags.clone(),
        color: host.color.clone(),
    }
}
