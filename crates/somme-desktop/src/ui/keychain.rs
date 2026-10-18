use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use somme_application::dto::{IdentityInput, IdentityView, KeyImportInput, KeyView, SecretUpdate};
use somme_domain::Id;

use crate::store::{Store, mutate, mutate_weak, report};
use crate::ui::common::{
    Lucide, card, confirm_delete, copy_text, empty_state, form_dialog, host_badge, labeled, notify_error,
    page_header, subtle,
};
use crate::ui::fields::{ChoiceState, choice_select, key_choices, selected};

/// SSH keys and identities (saved user name + password/key pairs).
pub struct KeychainPage {
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl KeychainPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search keys and identities"));
        let subscriptions = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            search,
            _subscriptions: subscriptions,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn rename_key(&mut self, key: KeyView, window: &mut Window, cx: &mut Context<Self>) {
        let label = cx.new(|cx| InputState::new(window, cx).default_value(key.label.clone()));
        let page = cx.entity().downgrade();
        let focus = label.focus_handle(cx);
        form_dialog(
            "Rename Key",
            "Save",
            400.,
            Some(focus),
            {
                let label = label.clone();
                move |_, _| Input::new(&label).into_any_element()
            },
            move |window, cx| {
                let text = label.read(cx).value().trim().to_owned();
                if text.is_empty() {
                    return false;
                }
                let id = key.id;
                mutate_weak(
                    &page,
                    window,
                    cx,
                    move |services| async move { services.keys.rename(id, &text).await },
                    report("Could not rename key"),
                );
                true
            },
            window,
            cx,
        );
    }

    fn delete_key(&mut self, key: KeyView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = key.id;
        confirm_delete(
            format!("Delete key {}?", key.label),
            "Hosts and identities that use it will stop using it. This cannot be undone.",
            move |window, cx| {
                mutate_weak(
                    &page,
                    window,
                    cx,
                    move |services| async move { services.keys.delete(id).await },
                    report("Could not delete key"),
                );
            },
            window,
            cx,
        );
    }

    fn delete_identity(&mut self, identity: IdentityView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = identity.id;
        confirm_delete(
            format!("Delete identity {}?", identity.label),
            "Hosts that use it will stop using it. This cannot be undone.",
            move |window, cx| {
                mutate_weak(
                    &page,
                    window,
                    cx,
                    move |services| async move { services.identities.delete(id).await },
                    report("Could not delete identity"),
                );
            },
            window,
            cx,
        );
    }

    // ---- rendering ----

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        page_header("Keychain", cx)
            .child(
                div().w(px(280.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(
                Button::new("import-key")
                    .primary()
                    .small()
                    .icon(Icon::new(Lucide::FileKey))
                    .label("Import Key")
                    .on_click(|_, window, cx| KeyImport::open(window, cx)),
            )
            .child(
                Button::new("new-identity")
                    .small()
                    .icon(Icon::new(Lucide::UserPlus))
                    .label("New Identity")
                    .on_click(|_, window, cx| IdentityEditor::open(None, window, cx)),
            )
    }

    fn render_key(&self, key: KeyView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = key.id;
        let page = cx.entity().downgrade();
        let public_key = key.public_key.clone();
        let menu_key = key.clone();
        card(cx)
            .id(SharedString::from(format!("key-{id}")))
            .group("key")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(None, Icon::new(Lucide::KeyRound), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(key.label.clone()))
                    .child(subtle(format!("{} · {}", key_type(&key.algorithm), key.fingerprint), cx)),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("key", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("copy-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy public key")
                            .on_click(move |_: &ClickEvent, _, cx| {
                                copy_text(public_key.clone(), cx);
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("key-menu-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu(move |menu, _, _| {
                                let copy = menu_key.public_key.clone();
                                let rename = (page.clone(), menu_key.clone());
                                let delete = (page.clone(), menu_key.clone());
                                menu.item(PopupMenuItem::new("Copy Public Key").icon(IconName::Copy).on_click(
                                    move |_, _, cx| copy_text(copy.clone(), cx),
                                ))
                                .item(PopupMenuItem::new("Rename").icon(Icon::new(Lucide::Pencil)).on_click(
                                    move |_, window, cx| {
                                        let (page, key) = rename.clone();
                                        let _ = page.update(cx, |page, cx| page.rename_key(key, window, cx));
                                    },
                                ))
                                .separator()
                                .item(PopupMenuItem::new("Delete").icon(IconName::Delete).on_click(
                                    move |_, window, cx| {
                                        let (page, key) = delete.clone();
                                        let _ = page.update(cx, |page, cx| page.delete_key(key, window, cx));
                                    },
                                ))
                            }),
                    ),
            )
    }

    fn render_identity(&self, identity: IdentityView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = identity.id;
        let page = cx.entity().downgrade();
        let key_label = identity
            .key_id
            .and_then(|k| Store::global(cx).read(cx).key(k).map(|k| k.label.clone()));
        let mut parts = vec![identity.username.clone()];
        if identity.has_password {
            parts.push("password".into());
        }
        if let Some(key) = key_label {
            parts.push(format!("key {key}"));
        }
        let edit = identity.clone();
        let delete = identity.clone();
        card(cx)
            .id(SharedString::from(format!("identity-{id}")))
            .group("identity")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(None, Icon::new(Lucide::CircleUser), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(identity.label.clone()))
                    .child(subtle(parts.join(" · "), cx)),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("identity", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("identity-delete-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Delete")
                            .on_click(move |_: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                let _ = page.update(cx, |page, cx| page.delete_identity(delete.clone(), window, cx));
                            }),
                    ),
            )
            .on_click(move |_, window, cx| IdentityEditor::open(Some(edit.clone()), window, cx))
    }
}

/// "ssh-ed25519" reads better as "ED25519".
fn key_type(algorithm: &str) -> String {
    algorithm.trim_start_matches("ssh-").to_uppercase()
}

fn section(title: &str, count: usize, grid: impl IntoElement, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(format!("{title} ({count})")),
        )
        .child(grid)
}

impl Render for KeychainPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);
        let store = Store::global(cx);
        let (keys, identities) = {
            let store = store.read(cx);
            let keys: Vec<KeyView> = store
                .keys
                .iter()
                .filter(|k| {
                    query.is_empty()
                        || k.label.to_lowercase().contains(&query)
                        || k.algorithm.to_lowercase().contains(&query)
                        || k.fingerprint.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            let identities: Vec<IdentityView> = store
                .identities
                .iter()
                .filter(|i| {
                    query.is_empty()
                        || i.label.to_lowercase().contains(&query)
                        || i.username.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            (keys, identities)
        };
        let nothing = store.read(cx).keys.is_empty() && store.read(cx).identities.is_empty();

        let mut body = v_flex().px_5().py_4().gap_5();
        if nothing {
            body = body.child(
                empty_state(
                    Icon::new(Lucide::KeyRound),
                    "No keys yet",
                    "Import an SSH key you already have. Identities save a user name with a password or key.",
                    cx,
                )
                .child(
                    h_flex()
                        .gap_2()
                        .pt_2()
                        .child(
                            Button::new("empty-import")
                                .primary()
                                .icon(Icon::new(Lucide::FileKey))
                                .label("Import Key")
                                .on_click(|_, window, cx| KeyImport::open(window, cx)),
                        )
                        .child(
                            Button::new("empty-identity")
                                .icon(Icon::new(Lucide::UserPlus))
                                .label("New Identity")
                                .on_click(|_, window, cx| IdentityEditor::open(None, window, cx)),
                        ),
                ),
            );
        } else {
            let key_count = keys.len();
            let identity_count = identities.len();
            let key_grid = div()
                .grid()
                .grid_cols(3)
                .gap_3()
                .children(keys.into_iter().map(|k| self.render_key(k, cx)));
            let identity_grid = if identities.is_empty() && query.is_empty() {
                h_flex()
                    .gap_2()
                    .child(subtle("No identities yet. They save a user name with a password or key.", cx))
                    .child(
                        Button::new("add-identity")
                            .link()
                            .xsmall()
                            .label("New Identity")
                            .on_click(|_, window, cx| IdentityEditor::open(None, window, cx)),
                    )
                    .into_any_element()
            } else {
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_3()
                    .children(identities.into_iter().map(|i| self.render_identity(i, cx)))
                    .into_any_element()
            };
            body = body
                .child(section("Keys", key_count, key_grid, cx))
                .child(section("Identities", identity_count, identity_grid, cx));
        }

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}

// ---- import ----

/// The side sheet for importing an existing private key.
struct KeyImport {
    label: Entity<InputState>,
    private_key: Entity<TextareaState>,
    passphrase: Entity<InputState>,
    error: Option<String>,
    saving: bool,
}

/// SSH private keys are a few KB at most; anything bigger is the wrong file.
const MAX_KEY_FILE: u64 = 64 * 1024;

fn read_key_file(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_KEY_FILE {
        return Err("This file is too big to be an SSH key.".into());
    }
    std::fs::read_to_string(path).map_err(|_| "This file is not a text key file.".to_owned())
}

impl KeyImport {
    fn open(window: &mut Window, cx: &mut App) {
        let editor = cx.new(|cx| Self {
            label: cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Work key")),
            private_key: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(10)
                    .placeholder("-----BEGIN OPENSSH PRIVATE KEY-----")
            }),
            passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Only if the key has one")
            }),
            error: None,
            saving: false,
        });
        let first = std::cell::Cell::new(true);
        window.open_sheet(cx, move |sheet, window, cx| {
            if first.replace(false) {
                let input = editor.read(cx).label.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            let save = editor.clone();
            sheet
                .title("Import Key")
                .size(px(460.))
                .child(editor.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("import-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_sheet(cx)),
                        )
                        .child(
                            Button::new("import-save")
                                .primary()
                                .label("Import")
                                .loading(editor.read(cx).saving)
                                .on_click(move |_, window, cx| save.update(cx, |e, cx| e.save(window, cx))),
                        ),
                )
        });
    }

    fn load_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let text = read_key_file(&path);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                match text {
                    Ok(text) => {
                        this.error = None;
                        this.private_key.update(cx, |input, cx| input.set_value(text, window, cx));
                        if this.label.read(cx).value().trim().is_empty() {
                            this.label.update(cx, |input, cx| input.set_value(name, window, cx));
                        }
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let private_key = self.private_key.read(cx).value().trim().to_owned();
        if private_key.is_empty() {
            self.error = Some("Paste a private key or load it from a file.".into());
            cx.notify();
            return;
        }
        let mut label = self.label.read(cx).value().trim().to_owned();
        if label.is_empty() {
            label = "Imported key".into();
        }
        let passphrase = Some(self.passphrase.read(cx).value().to_string()).filter(|p| !p.is_empty());
        let input = KeyImportInput {
            label,
            private_key,
            passphrase,
        };
        self.saving = true;
        self.error = None;
        cx.notify();
        mutate(
            window,
            cx,
            move |services| async move { services.keys.import(input).await },
            |editor, result, window, cx| {
                editor.saving = false;
                match result {
                    Ok(_) => window.close_sheet(cx),
                    Err(err) => editor.error = Some(err.to_string()),
                }
                cx.notify();
            },
        );
    }
}

impl Render for KeyImport {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .gap_3()
            .overflow_y_scrollbar()
            .child(labeled("Label", Input::new(&self.label)))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().child("Private key"))
                            .child(
                                Button::new("load-key-file")
                                    .link()
                                    .xsmall()
                                    .label("Load from file…")
                                    .on_click(cx.listener(|editor, _, window, cx| editor.load_file(window, cx))),
                            ),
                    )
                    .child(div().font_family(theme.mono_font_family.clone()).child(Textarea::new(&self.private_key))),
            )
            .child(labeled("Passphrase", Input::new(&self.passphrase).mask_toggle()))
            .child(subtle("The key is encrypted with your master password.", cx))
            .when_some(self.error.clone(), |col, err| {
                col.child(div().text_sm().text_color(theme.danger).child(err))
            })
    }
}

// ---- identities ----

/// The form for adding or changing an identity, shown in a dialog.
struct IdentityEditor {
    id: Option<Id>,
    had_password: bool,
    clear_password: bool,
    label: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    key: ChoiceState<Option<Id>>,
}

impl IdentityEditor {
    fn open(identity: Option<IdentityView>, window: &mut Window, cx: &mut App) {
        let editor = cx.new(|cx| Self::new(identity, window, cx));
        let (title, focus) = {
            let e = editor.read(cx);
            let title = if e.id.is_some() { "Edit Identity" } else { "New Identity" };
            (title, e.label.focus_handle(cx))
        };
        let body = editor.clone();
        form_dialog(
            title,
            "Save",
            440.,
            Some(focus),
            move |_, _| body.clone().into_any_element(),
            move |window, cx| editor.update(cx, |e, cx| e.save(window, cx)),
            window,
            cx,
        );
    }

    fn new(identity: Option<IdentityView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let i = identity.as_ref();
        let had_password = i.is_some_and(|i| i.has_password);
        let key_id = i.and_then(|i| i.key_id);
        Self {
            id: i.map(|i| i.id),
            had_password,
            clear_password: false,
            label: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("e.g. Deploy user")
                    .default_value(i.map(|i| i.label.clone()).unwrap_or_default())
            }),
            username: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("root")
                    .default_value(i.map(|i| i.username.clone()).unwrap_or_default())
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(if had_password { "Saved — type to replace" } else { "Optional" })
            }),
            key: choice_select(key_choices(cx), &key_id, window, cx),
        }
    }

    /// Returns true when the dialog can close.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let username = self.username.read(cx).value().trim().to_owned();
        if username.is_empty() {
            notify_error(window, cx, "Could not save identity", "Enter a user name.");
            return false;
        }
        let mut label = self.label.read(cx).value().trim().to_owned();
        if label.is_empty() {
            label = username.clone();
        }
        let typed = self.password.read(cx).value().to_string();
        let password = if !typed.is_empty() {
            SecretUpdate::Set(typed)
        } else if self.clear_password {
            SecretUpdate::Clear
        } else {
            SecretUpdate::Keep
        };
        let input = IdentityInput {
            id: self.id,
            label,
            username,
            password,
            key_id: selected(&self.key, cx).flatten(),
        };
        mutate(
            window,
            cx,
            move |services| async move { services.identities.save(input).await },
            report("Could not save identity"),
        );
        true
    }
}

impl Render for IdentityEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(labeled("Label", Input::new(&self.label)))
            .child(labeled("User name", Input::new(&self.username)))
            .child(
                labeled("Password", Input::new(&self.password).mask_toggle()).when(self.had_password, |col| {
                    col.child(
                        h_flex().justify_end().child(
                            Button::new("identity-clear-password")
                                .link()
                                .xsmall()
                                .label(if self.clear_password { "Password will be removed" } else { "Remove saved password" })
                                .on_click(cx.listener(|editor, _, _, cx| {
                                    editor.clear_password = !editor.clear_password;
                                    cx.notify();
                                })),
                        ),
                    )
                }),
            )
            .child(labeled("SSH Key", Select::new(&self.key)))
    }
}
