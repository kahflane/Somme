use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _,
};

use crate::settings::{Appearance, BellSetting, CursorShapeSetting, Settings};
use crate::store::{Store, mutate_weak, report};
use crate::terminal::colors::THEMES;
use crate::ui::common::{Lucide, ago, form_dialog, labeled, notify_error, notify_ok};

/// App and terminal preferences, security, and the sync account.
pub struct SettingsPage {
    fonts: Vec<SharedString>,
    /// The settings view keeps its selected page in keyed state, so a new key
    /// is the only way to jump to another page from outside.
    visit: usize,
    start_page: usize,
}

const ACCOUNT_PAGE: usize = 3;

impl SettingsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Scanning system fonts takes a moment, so the list fills in when ready.
        cx.spawn(async move |page, cx| {
            let fonts = crate::runtime::io(async { Ok::<_, std::convert::Infallible>(monospace_fonts()) }).await;
            let _ = page.update(cx, |page, cx| {
                page.fonts = fonts.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
        Self {
            fonts: Vec::new(),
            visit: 0,
            start_page: 0,
        }
    }

    pub fn show_account(&mut self, cx: &mut Context<Self>) {
        self.visit += 1;
        self.start_page = ACCOUNT_PAGE;
        cx.notify();
    }
}

/// The scrolling menu shows a single row, so it is only used for long lists.
fn font_field(options: Vec<(SharedString, SharedString)>) -> SettingField<SharedString> {
    let get = |cx: &App| -> SharedString { Settings::get(cx).font_family.clone().into() };
    let set = |name: SharedString, cx: &mut App| Settings::update(cx, |s| s.font_family = name.to_string());
    if options.len() > 20 {
        SettingField::scrollable_dropdown(options, get, set)
    } else {
        SettingField::dropdown(options, get, set)
    }
}

fn monospace_fonts() -> Vec<SharedString> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut names: Vec<String> = db
        .faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .filter(|name| !name.starts_with('.') && !name.contains("Bitmap"))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names.into_iter().map(SharedString::from).collect()
}

/// A switch bound to one settings field.
fn switch(get: fn(&Settings) -> bool, set: fn(&mut Settings, bool)) -> SettingField<bool> {
    SettingField::switch(move |cx| get(Settings::get(cx)), move |on, cx| Settings::update(cx, |s| set(s, on)))
        .default_value(get(&Settings::default()))
}

/// A dropdown over a fixed list of values, stored as an enum or number.
fn choice<T: PartialEq + Copy + 'static>(
    options: &'static [(T, &'static str)],
    get: fn(&Settings) -> T,
    set: fn(&mut Settings, T),
) -> SettingField<SharedString> {
    choice_then(options, get, set, |_| {})
}

/// `choice` that also runs `after` once the new value is saved.
fn choice_then<T: PartialEq + Copy + 'static>(
    options: &'static [(T, &'static str)],
    get: fn(&Settings) -> T,
    set: fn(&mut Settings, T),
    after: fn(&mut App),
) -> SettingField<SharedString> {
    let key = |ix: usize| SharedString::from(ix.to_string());
    let items = options.iter().enumerate().map(|(ix, (_, label))| (key(ix), SharedString::from(*label))).collect();
    let index_of = move |value: T| options.iter().position(|(v, _)| *v == value).unwrap_or(0);
    SettingField::dropdown(
        items,
        move |cx| key(index_of(get(Settings::get(cx)))),
        move |picked, cx| {
            if let Some((value, _)) = picked.parse::<usize>().ok().and_then(|ix| options.get(ix)) {
                let value = *value;
                Settings::update(cx, |s| set(s, value));
                after(cx);
            }
        },
    )
    .default_value(key(index_of(get(&Settings::default()))))
}

const APPEARANCES: &[(Appearance, &str)] =
    &[(Appearance::Dark, "Dark"), (Appearance::Light, "Light"), (Appearance::System, "Match system")];
const CURSORS: &[(CursorShapeSetting, &str)] = &[
    (CursorShapeSetting::Block, "Block"),
    (CursorShapeSetting::Bar, "Bar"),
    (CursorShapeSetting::Underline, "Underline"),
];
const BELLS: &[(BellSetting, &str)] =
    &[(BellSetting::Visual, "Flash the screen"), (BellSetting::Sound, "Play a sound"), (BellSetting::Off, "Off")];
const AUTO_LOCK: &[(u32, &str)] = &[
    (0, "Never"),
    (1, "After 1 minute"),
    (5, "After 5 minutes"),
    (15, "After 15 minutes"),
    (30, "After 30 minutes"),
    (60, "After 1 hour"),
];

fn appearance_page(fonts: &[SharedString], cx: &App) -> SettingPage {
    let mut font_options: Vec<(SharedString, SharedString)> = fonts.iter().map(|f| (f.clone(), f.clone())).collect();
    let current: SharedString = Settings::get(cx).font_family.clone().into();
    if !fonts.contains(&current) {
        font_options.insert(0, (current.clone(), current));
    }
    let themes = THEMES.iter().map(|t| (SharedString::from(t.name), SharedString::from(t.name))).collect();
    let defaults = Settings::default();
    SettingPage::new("Appearance")
        .icon(Icon::new(Lucide::Palette))
        .default_open(true)
        .group(
            SettingGroup::new().title("Window").item(SettingItem::new(
                "Theme",
                // The UI colors switch too, not just the saved setting.
                choice_then(APPEARANCES, |s| s.appearance, |s, v| s.appearance = v, crate::apply_appearance),
            )),
        )
        .group(
            SettingGroup::new()
                .title("Terminal")
                .item(
                    SettingItem::new(
                        "Color scheme",
                        SettingField::dropdown(
                            themes,
                            |cx| Settings::get(cx).terminal_theme.clone().into(),
                            |name, cx| Settings::update(cx, |s| s.terminal_theme = name.to_string()),
                        )
                        .default_value(SharedString::from(defaults.terminal_theme.clone())),
                    )
                    .description("Colors for text, background and the 16 ANSI colors."),
                )
                .item(
                    SettingItem::new(
                        "Font",
                        font_field(font_options).default_value(SharedString::from(defaults.font_family.clone())),
                    )
                    .description("Only fixed-width fonts are listed."),
                )
                .item(SettingItem::new(
                    "Font size",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 8.,
                            max: 32.,
                            step: 1.,
                        },
                        |cx| Settings::get(cx).font_size as f64,
                        |size, cx| Settings::update(cx, |s| s.font_size = size.clamp(8., 32.) as f32),
                    )
                    .default_value(defaults.font_size as f64),
                ))
                .item(SettingItem::new(
                    "Line height",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 0.8,
                            max: 3.,
                            step: 0.05,
                        },
                        |cx| (Settings::get(cx).line_height as f64 * 100.).round() / 100.,
                        |height, cx| Settings::update(cx, |s| s.line_height = height.clamp(0.8, 3.) as f32),
                    )
                    .default_value(defaults.line_height as f64),
                )),
        )
}

fn terminal_page() -> SettingPage {
    SettingPage::new("Terminal").icon(Icon::new(Lucide::SquareTerminal)).group(
        SettingGroup::new()
            .title("Behavior")
            .item(SettingItem::new(
                "Cursor shape",
                choice(CURSORS, |s| s.cursor_shape, |s, v| s.cursor_shape = v),
            ))
            .item(SettingItem::new("Blinking cursor", switch(|s| s.cursor_blink, |s, v| s.cursor_blink = v)))
            .item(
                SettingItem::new(
                    "Scrollback lines",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 1000.,
                            max: 100_000.,
                            step: 1000.,
                        },
                        |cx| Settings::get(cx).scrollback_lines as f64,
                        |lines, cx| {
                            Settings::update(cx, |s| s.scrollback_lines = lines.clamp(1000., 100_000.) as usize)
                        },
                    )
                    .default_value(Settings::default().scrollback_lines as f64),
                )
                .description("How many lines you can scroll back. Applies to new terminals."),
            )
            .item(SettingItem::new("Bell", choice(BELLS, |s| s.bell, |s, v| s.bell = v)))
            .item(
                SettingItem::new("Copy on select", switch(|s| s.copy_on_select, |s, v| s.copy_on_select = v))
                    .description("Selected text goes to the clipboard right away."),
            )
            .item(SettingItem::new(
                "Paste on right-click",
                switch(|s| s.paste_on_right_click, |s, v| s.paste_on_right_click = v),
            ))
            .items(cfg!(target_os = "macos").then(|| {
                SettingItem::new("Use Option as Meta", switch(|s| s.option_as_meta, |s, v| s.option_as_meta = v))
                    .description("Needed for Alt shortcuts in Emacs, tmux and shells.")
            }))
            .item(
                SettingItem::new(
                    "Ask before closing a running tab",
                    switch(|s| s.confirm_close_tab, |s, v| s.confirm_close_tab = v),
                ),
            ),
    )
}

fn security_page() -> SettingPage {
    SettingPage::new("Security").icon(Icon::new(Lucide::Lock)).group(
        SettingGroup::new()
            .title("Vault")
            .item(
                SettingItem::new(
                    "Lock automatically",
                    choice(AUTO_LOCK, |s| s.auto_lock_minutes, |s, v| s.auto_lock_minutes = v),
                )
                .description("Locks the vault when you have not used Somme for a while."),
            )
            .item(SettingItem::render(|_, _, cx| {
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .child(div().text_sm().child("Lock now"))
                            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Shortcut: ⌘⇧L")),
                    )
                    .child(
                        Button::new("lock-now")
                            .small()
                            .icon(Icon::new(Lucide::Lock))
                            .label("Lock")
                            .on_click(|_, _, cx| Store::global(cx).update(cx, |store, cx| store.lock(cx))),
                    )
            }))
            .item(
                SettingItem::new("Keep command history", switch(|s| s.keep_history, |s, v| s.keep_history = v))
                    .description("Remembers commands you type, for search and reuse."),
            )
            .item(
                SettingItem::new("Record session logs", switch(|s| s.log_sessions, |s, v| s.log_sessions = v))
                    .description("Saves what each terminal prints. Open them from Logs."),
            ),
    )
}

fn account_page(page: WeakEntity<SettingsPage>) -> SettingPage {
    SettingPage::new("Account").icon(Icon::new(Lucide::CircleUser)).group(
        SettingGroup::new()
            .title("Sync")
            .description("Sign in to keep hosts, keys and snippets in sync across your devices. Data is encrypted with your master password before it leaves this device.")
            .item(SettingItem::render(move |_, window, cx| account_panel(page.clone(), window, cx))),
    )
}

fn account_panel(page: WeakEntity<SettingsPage>, _: &mut Window, cx: &mut App) -> AnyElement {
    let store = Store::global(cx);
    let (account, sync) = {
        let store = store.read(cx);
        (store.account.clone(), store.sync.clone())
    };
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    if !account.signed_in {
        let sign_in = page.clone();
        let register = page;
        return h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(div().text_sm().child("Not signed in"))
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("create-account").small().label("Create Account").on_click(
                        move |_, window, cx| open_auth(register.clone(), true, window, cx),
                    ))
                    .child(Button::new("sign-in").small().primary().label("Sign In").on_click(
                        move |_, window, cx| open_auth(sign_in.clone(), false, window, cx),
                    )),
            )
            .into_any_element();
    }

    let status = if sync.syncing {
        "Syncing…".to_owned()
    } else if let Some(at) = sync.last_sync_at {
        format!("Last synced {}", ago(at))
    } else {
        "Not synced yet".to_owned()
    };
    let sync_page = page.clone();
    let out_page = page;
    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_sm().font_medium().truncate().child(account.email.unwrap_or_default()))
                        .child(div().text_xs().text_color(muted).child(status)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .when(sync.syncing, |row| row.child(Spinner::new().small()))
                        .child(
                            Button::new("sync-now")
                                .small()
                                .icon(Icon::new(Lucide::RefreshCw))
                                .label("Sync Now")
                                .disabled(sync.syncing)
                                .on_click(move |_, window, cx| {
                                    mutate_weak(
                                        &sync_page,
                                        window,
                                        cx,
                                        |services| async move { services.sync.sync_now().await },
                                        report("Sync failed"),
                                    )
                                }),
                        )
                        .child(Button::new("sign-out").small().label("Sign Out").on_click(move |_, window, cx| {
                            sign_out(out_page.clone(), window, cx)
                        })),
                ),
        )
        .when_some(sync.last_error, |panel, error| {
            panel.child(div().text_xs().text_color(danger).child(error))
        })
        .into_any_element()
}

fn sign_out(page: WeakEntity<SettingsPage>, window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let page = page.clone();
        alert
            .confirm()
            .title("Sign out?")
            .description("Sync stops on this device. Your data stays here and in the cloud.")
            .ok_text("Sign Out")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                mutate_weak(
                    &page,
                    window,
                    cx,
                    |services| async move { services.sync.logout().await },
                    |_, result, window, cx| match result {
                        Ok(_) => Store::global(cx).update(cx, |store, cx| store.refresh_account(cx)),
                        Err(err) => notify_error(window, cx, "Could not sign out", err),
                    },
                );
                true
            })
    });
}

fn open_auth(page: WeakEntity<SettingsPage>, register: bool, window: &mut Window, cx: &mut App) {
    let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
    let password = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
    let focus = email.focus_handle(cx);
    let (title, ok) = if register { ("Create Account", "Create Account") } else { ("Sign In", "Sign In") };
    form_dialog(
        title,
        ok,
        400.,
        Some(focus),
        {
            let email = email.clone();
            let password = password.clone();
            move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(labeled("Email", Input::new(&email)))
                    .child(labeled("Password", Input::new(&password).mask_toggle()))
                    .when(register, |form| {
                        form.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("This is your sync account password. Your vault stays locked by your master password."),
                        )
                    })
                    .into_any_element()
            }
        },
        move |window, cx| {
            let address = email.read(cx).value().trim().to_owned();
            let secret = password.read(cx).value().to_string();
            if address.is_empty() || secret.is_empty() {
                return false;
            }
            mutate_weak(
                &page,
                window,
                cx,
                move |services| async move {
                    if register {
                        services.sync.register(&address, &secret).await
                    } else {
                        services.sync.login(&address, &secret).await
                    }
                },
                move |_, result, window, cx| match result {
                    Ok(account) => {
                        Store::global(cx).update(cx, |store, cx| store.refresh_account(cx));
                        notify_ok(window, cx, format!("Signed in as {}", account.email.unwrap_or_default()));
                    }
                    Err(err) => notify_error(
                        window,
                        cx,
                        if register { "Could not create account" } else { "Could not sign in" },
                        err,
                    ),
                },
            );
            true
        },
        window,
        cx,
    );
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = cx.entity().downgrade();
        div().size_full().child(
            SettingsView::new(("app-settings", self.visit))
                .default_selected_index(SelectIndex {
                    page_ix: self.start_page,
                    group_ix: None,
                })
                .sidebar_width(gpui_kit::px(220.))
                .pages([appearance_page(&self.fonts, cx), terminal_page(), security_page(), account_page(page)]),
        )
    }
}
