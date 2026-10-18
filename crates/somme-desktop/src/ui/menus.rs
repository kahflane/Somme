//! The menu bar: Somme, File, Edit, View, Tab, Blocks, SSH, Window and Help.
//!
//! Most items run the same actions as their keyboard shortcuts. Edit items press
//! the shortcut instead, so they reach whatever has focus: a terminal, the
//! command editor, the file editor or a text box.

use gpui_kit::{
    App, KeyBinding, Keystroke, Menu, MenuItem, OsAction, PromptButton, PromptLevel, SystemMenuType, Window, actions,
};

use crate::terminal::view::{
    Clear, CopyLastCommand, CopyLastOutput, ScrollToBottom, ScrollToTop, ZoomIn, ZoomOut, ZoomReset,
};
use crate::ui::common::APP_NAME;
use crate::ui::native;
use crate::ui::workspace::{
    CloseTab, ClosePane, FocusNextPane, FocusPreviousPane, LockVault, NewEditor, NewLocalTerminal, NextTab, OpenFile,
    OpenSettings,
    PreviousTab, Quit, RenameTab, ShowForwarding, ShowHosts, ShowKeychain, ShowKnownHosts, ShowLogs,
    ShowSnippets, SplitDown, SplitLeft, SplitRight, SplitUp, ToggleBroadcast, TogglePalette, ToggleSidePanel,
};

actions!(
    menu,
    [
        About,
        Hide,
        HideOthers,
        ShowAll,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Find,
        FindNext,
        FindPrevious,
        Minimize,
        Zoom,
        ToggleFullScreen,
        OpenRepository,
        ReportIssue,
    ]
);

/// Only shows shortcuts beside menu items. Nothing ever has this context, so
/// the real keys keep going to whatever has focus.
const MENU_ONLY: &str = "MenuOnly";
const REPOSITORY: &str = "https://github.com/kahflane/Somme";

/// What each Edit item presses. Only macOS shows this menu bar.
const UNDO: &str = "cmd-z";
const REDO: &str = "cmd-shift-z";
const CUT: &str = "cmd-x";
const COPY: &str = "cmd-c";
const PASTE: &str = "cmd-v";
const SELECT_ALL: &str = "cmd-a";
const FIND: &str = "cmd-f";
const FIND_NEXT: &str = "cmd-g";
const FIND_PREVIOUS: &str = "cmd-shift-g";

pub fn init(cx: &mut App) {
    // Elsewhere ctrl-h and ctrl-m belong to the terminal.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        KeyBinding::new(UNDO, Undo, Some(MENU_ONLY)),
        KeyBinding::new(REDO, Redo, Some(MENU_ONLY)),
        KeyBinding::new(CUT, Cut, Some(MENU_ONLY)),
        KeyBinding::new(COPY, Copy, Some(MENU_ONLY)),
        KeyBinding::new(PASTE, Paste, Some(MENU_ONLY)),
        KeyBinding::new(SELECT_ALL, SelectAll, Some(MENU_ONLY)),
        KeyBinding::new(FIND, Find, Some(MENU_ONLY)),
        KeyBinding::new(FIND_NEXT, FindNext, Some(MENU_ONLY)),
        KeyBinding::new(FIND_PREVIOUS, FindPrevious, Some(MENU_ONLY)),
    ]);

    cx.on_action(|_: &About, cx| {
        with_window(cx, |window, cx| {
            let detail = format!("Version {}", env!("CARGO_PKG_VERSION"));
            drop(native::ask(PromptLevel::Info, APP_NAME, Some(&detail), &[PromptButton::ok("OK")], window, cx));
        })
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| with_window(cx, |window, _| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_window(cx, |window, _| window.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| with_window(cx, |window, _| window.toggle_fullscreen()));
    cx.on_action(|_: &OpenRepository, cx| cx.open_url(REPOSITORY));
    cx.on_action(|_: &ReportIssue, cx| cx.open_url(&format!("{REPOSITORY}/issues")));
    cx.on_action(|_: &Undo, cx| press(UNDO, cx));
    cx.on_action(|_: &Redo, cx| press(REDO, cx));
    cx.on_action(|_: &Cut, cx| press(CUT, cx));
    cx.on_action(|_: &Copy, cx| press(COPY, cx));
    cx.on_action(|_: &Paste, cx| press(PASTE, cx));
    cx.on_action(|_: &SelectAll, cx| press(SELECT_ALL, cx));
    cx.on_action(|_: &Find, cx| press(FIND, cx));
    cx.on_action(|_: &FindNext, cx| press(FIND_NEXT, cx));
    cx.on_action(|_: &FindPrevious, cx| press(FIND_PREVIOUS, cx));

    cx.set_menus(menus());
}

/// Runs `f` on the front window, or the only one when the app is not in
/// front. Menu actions arrive while that window is busy handling them, so this
/// waits until it is free.
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(|cx| {
        if let Some(handle) = cx.active_window().or_else(|| cx.windows().first().copied()) {
            let _ = handle.update(cx, |_, window, cx| f(window, cx));
        }
    });
}

/// Presses a shortcut in the front window, as if typed.
fn press(keys: &str, cx: &mut App) {
    let Ok(keystroke) = Keystroke::parse(keys) else { return };
    with_window(cx, |window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    });
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new(APP_NAME).items([
            MenuItem::action(format!("About {APP_NAME}"), About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(format!("Hide {APP_NAME}"), Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {APP_NAME}"), Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Tab", NewLocalTerminal),
            MenuItem::action("New Text Editor", NewEditor),
            MenuItem::action("Open File…", OpenFile),
            MenuItem::separator(),
            MenuItem::action("Split Right", SplitRight),
            MenuItem::action("Split Left", SplitLeft),
            MenuItem::action("Split Down", SplitDown),
            MenuItem::action("Split Up", SplitUp),
            MenuItem::separator(),
            MenuItem::action("Command Palette…", TogglePalette),
            MenuItem::separator(),
            MenuItem::action("Close Pane", ClosePane),
            MenuItem::action("Close Tab", CloseTab),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find…", Find),
            MenuItem::action("Find Next", FindNext),
            MenuItem::action("Find Previous", FindPrevious),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Side Panel", ToggleSidePanel),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Actual Size", ZoomReset),
            MenuItem::separator(),
            MenuItem::action("Enter Full Screen", ToggleFullScreen),
        ]),
        Menu::new("Tab").items([
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PreviousTab),
            MenuItem::separator(),
            MenuItem::action("Rename Tab…", RenameTab),
            MenuItem::action("Broadcast Input to All Panes", ToggleBroadcast),
            MenuItem::separator(),
            MenuItem::action("Next Pane", FocusNextPane),
            MenuItem::action("Previous Pane", FocusPreviousPane),
        ]),
        Menu::new("Blocks").items([
            MenuItem::action("Copy Last Command", CopyLastCommand),
            MenuItem::action("Copy Last Output", CopyLastOutput),
            MenuItem::separator(),
            MenuItem::action("Scroll to Top", ScrollToTop),
            MenuItem::action("Scroll to Bottom", ScrollToBottom),
            MenuItem::separator(),
            MenuItem::action("Clear Blocks", Clear),
        ]),
        Menu::new("SSH").items([
            MenuItem::action("Hosts", ShowHosts),
            MenuItem::action("Keychain", ShowKeychain),
            MenuItem::action("Snippets", ShowSnippets),
            MenuItem::action("Port Forwarding", ShowForwarding),
            MenuItem::action("Known Hosts", ShowKnownHosts),
            MenuItem::action("Logs", ShowLogs),
            MenuItem::separator(),
            MenuItem::action("Lock Saved Servers and Keys", LockVault),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        Menu::new("Help").items([
            MenuItem::action(format!("{APP_NAME} on GitHub"), OpenRepository),
            MenuItem::action("Report an Issue", ReportIssue),
        ]),
    ]
}
