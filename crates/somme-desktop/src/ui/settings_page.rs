use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};

use crate::settings::{Appearance, BellSetting, CursorShapeSetting, Settings};
use crate::store::{Store, mutate_weak, report};
use crate::ui::common::{APP_NAME, Lucide, ago, avatar, notify_error, notify_ok};
use crate::ui::native;
use crate::ui::theme_picker::{self, section};

/// App and terminal preferences, security, and the sync account.
pub struct SettingsPage {
    fonts: Vec<SharedString>,
    /// The settings view keeps its selected page in keyed state, so a new key
    /// is the only way to jump to another page from outside.
    visit: usize,
    start_page: usize,
}

const ACCOUNT_PAGE: usize = 5;

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

pub(crate) fn monospace_fonts() -> Vec<SharedString> {
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
    let defaults = Settings::default();
    SettingPage::new("Appearance")
        .icon(Icon::new(Lucide::Palette))
        .default_open(true)
        .group(
            SettingGroup::new()
                .title("App")
                .item(
                    SettingItem::new(
                        "Mode",
                        // The UI colors switch too, not just the saved setting.
                        choice_then(APPEARANCES, |s| s.appearance, |s, v| s.appearance = v, crate::apply_appearance),
                    )
                    .description("Each mode keeps its own theme."),
                )
                .item(
                    SettingItem::render(|_, _, cx| section("Dark theme", theme_picker::app_themes(true, cx), cx))
                        .keywords(["theme", "color", "dark"])
                        .on_reset(
                            |cx| Settings::get(cx).dark_theme != Settings::default().dark_theme,
                            |_, cx| {
                                Settings::update(cx, |s| s.dark_theme = Settings::default().dark_theme);
                                crate::apply_appearance(cx);
                            },
                        ),
                )
                .item(
                    SettingItem::render(|_, _, cx| section("Light theme", theme_picker::app_themes(false, cx), cx))
                        .keywords(["theme", "color", "light"])
                        .on_reset(
                            |cx| Settings::get(cx).light_theme != Settings::default().light_theme,
                            |_, cx| {
                                Settings::update(cx, |s| s.light_theme = Settings::default().light_theme);
                                crate::apply_appearance(cx);
                            },
                        ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Terminal colors")
                .item(
                    SettingItem::new(
                        "Match app theme",
                        switch(|s| s.terminal_matches_theme, |s, v| s.terminal_matches_theme = v),
                    )
                    .description("Terminals use the colors of the app theme. Pick a scheme below to use other colors."),
                )
                .item(
                    SettingItem::render(|_, _, cx| section("Color scheme", theme_picker::terminal_schemes(cx), cx))
                        .keywords(["theme", "color", "scheme", "ansi"])
                        .on_reset(
                            |cx| {
                                let s = Settings::get(cx);
                                !s.terminal_matches_theme || s.terminal_theme != Settings::default().terminal_theme
                            },
                            |_, cx| {
                                Settings::update(cx, |s| {
                                    s.terminal_theme = Settings::default().terminal_theme;
                                    s.terminal_matches_theme = true;
                                })
                            },
                        ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Font")
                .item(
                    SettingItem::new(
                        "Font",
                        font_field(font_options).default_value(SharedString::from(defaults.font_family.clone())),
                    )
                    .description("Only fixed-width fonts are listed."),
                )
                .item(
                    SettingItem::new(
                        "Use for the whole app",
                        switch(|s| s.font_everywhere, |s, v| s.font_everywhere = v),
                    )
                    .description("When off, only terminals and code use this font."),
                )
                .item(
                    SettingItem::new(
                        "App font size",
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: 12.,
                                max: 24.,
                                step: 1.,
                            },
                            |cx| Settings::get(cx).ui_font_size as f64,
                            |size, cx| Settings::update(cx, |s| s.ui_font_size = size.clamp(12., 24.) as f32),
                        )
                        .default_value(defaults.ui_font_size as f64),
                    )
                    .description("Menus, lists and forms. Terminals use their own size below."),
                )
                .item(SettingItem::new(
                    "Terminal font size",
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
            .item(
                SettingItem::new("Command blocks", switch(|s| s.command_blocks, |s, v| s.command_blocks = v))
                    .description("Local terminals show each command as a block, with an editor to type in. Applies to new terminals."),
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
            .item(
                SettingItem::new(
                    "Let servers copy to clipboard",
                    switch(|s| s.remote_clipboard_write, |s, v| s.remote_clipboard_write = v),
                )
                .description("For copying from tmux or vim over SSH. A server can then replace what you copied."),
            )
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

// Short pages keep to one group: the sidebar lists a page's groups as jump
// links, which do nothing on a page too short to scroll.
fn editor_page() -> SettingPage {
    SettingPage::new("Editor").icon(Icon::new(Lucide::FilePen)).group(
        SettingGroup::new().title("Files").item(
            SettingItem::new("Reopen files on start", switch(|s| s.reopen_files, |s, v| s.reopen_files = v))
                .description(format!(
                    "Opens the files you had open when you last quit {APP_NAME}, unsaved new files too."
                )),
        ),
    )
}

fn security_page() -> SettingPage {
    SettingPage::new("Security")
        .icon(Icon::new(Lucide::Lock))
        .group(
            SettingGroup::new()
                .title("Master password")
                .description(
                    "Your saved servers, SSH keys and passwords are encrypted with your master password. When they are locked, Somme asks for it again before showing them. Open terminals keep running.",
                )
                .item(
                    SettingItem::new(
                        "Lock automatically",
                        choice(AUTO_LOCK, |s| s.auto_lock_minutes, |s, v| s.auto_lock_minutes = v),
                    )
                    .description(format!("Locks them when you have not used {APP_NAME} for a while.")),
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
                })),
        )
}

fn privacy_page() -> SettingPage {
    SettingPage::new("Privacy").icon(Icon::new(Lucide::EyeOff)).group(
        SettingGroup::new()
            .title("History and logs")
            .description(format!("What {APP_NAME} keeps on this computer."))
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
            .description("Sign in with GitHub to keep hosts, keys, snippets and themes in sync across your devices. Passwords and keys are encrypted with your master password before they leave this device.")
            .item(SettingItem::render(move |_, window, cx| account_panel(page.clone(), window, cx))),
    )
}

fn account_panel(page: WeakEntity<SettingsPage>, _: &mut Window, cx: &mut App) -> AnyElement {
    let store = Store::global(cx);
    let (account, sync, picture, signing_in) = {
        let store = store.read(cx);
        (store.account.clone(), store.sync.clone(), store.avatar.clone(), store.signing_in)
    };
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    if !account.signed_in {
        let row = h_flex().w_full().justify_between().items_center();
        if signing_in {
            return row
                .child(
                    h_flex()
                        .gap_2()
                        .child(Spinner::new().small())
                        .child(div().text_sm().child("Waiting for GitHub in your browser…")),
                )
                .child(
                    Button::new("cancel-sign-in")
                        .small()
                        .label("Cancel")
                        .on_click(|_, _, cx| Store::services(cx).sync.cancel_sign_in()),
                )
                .into_any_element();
        }
        return row
            .child(div().text_sm().child("Not signed in"))
            .child(
                Button::new("sign-in")
                    .small()
                    .primary()
                    .icon(Icon::new(Lucide::Github))
                    .label("Sign in with GitHub")
                    .on_click(move |_, window, cx| sign_in(page.clone(), window, cx)),
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
    let login = account.login.clone().unwrap_or_default();
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
                    h_flex()
                        .min_w_0()
                        .gap_3()
                        .child(avatar(picture, px(32.), cx))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(div().text_sm().font_medium().truncate().child(account.name.unwrap_or_else(|| login.clone())))
                                .child(div().text_xs().text_color(muted).truncate().child(format!("@{login} · {status}"))),
                        ),
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
                                        |services| async move {
                                            services.start_sync();
                                            services.sync.sync_now().await
                                        },
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
    native::confirm(
        "Sign out?",
        "Sync stops on this device. Your data stays here and in the cloud.",
        "Sign Out",
        move |window, cx| {
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
        },
        window,
        cx,
    );
}

fn sign_in(page: WeakEntity<SettingsPage>, window: &mut Window, cx: &mut App) {
    Store::global(cx).update(cx, |store, cx| {
        store.signing_in = true;
        cx.notify();
    });
    mutate_weak(
        &page,
        window,
        cx,
        |services| async move {
            services.start_sync();
            services.sync.sign_in_with_github().await
        },
        |_, result, window, cx| {
            // Joining an account that already has a vault locks this one until its
            // master password is entered.
            Store::global(cx).update(cx, |store, cx| {
                store.signing_in = false;
                store.refresh_account(cx);
                store.check_vault(cx);
            });
            match result {
                Ok(account) => {
                    cx.activate(true);
                    notify_ok(cx, format!("Signed in as {}", account.login.unwrap_or_default()));
                }
                Err(err) if err.kind() == "cancelled" => {}
                Err(err) => notify_error(window, cx, "Could not sign in", err),
            }
        },
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
                .pages([
                    appearance_page(&self.fonts, cx),
                    terminal_page(),
                    editor_page(),
                    security_page(),
                    privacy_page(),
                    account_page(page),
                ]),
        )
    }
}
