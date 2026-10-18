#[cfg(feature = "snapshot")]
mod automation;
mod changes;
mod editor;
mod github;
mod host_os;
mod look;
mod mcp;
mod runtime;
mod search;
mod services;
mod settings;
mod sftp;
mod store;
mod terminal;
mod ui;

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

use crate::services::{BackendEvent, Services};
use crate::settings::{Appearance, Settings, SettingsFile};
use crate::store::Store;
use crate::ui::workspace::Workspace;

pub(crate) fn apply_appearance(cx: &mut App) {
    ui::themes::install(cx);
    match Settings::get(cx).appearance {
        Appearance::Dark => Theme::change(ThemeMode::Dark, None, cx),
        Appearance::Light => Theme::change(ThemeMode::Light, None, cx),
        Appearance::System => Theme::sync_system_appearance(None, cx),
    }
    // The library's own themes keep the code colours of the last mode; match them up.
    let theme = Theme::global_mut(cx);
    if theme.highlight_theme.appearance.is_dark() != theme.mode.is_dark() {
        theme.highlight_theme = if theme.mode.is_dark() {
            HighlightTheme::default_dark()
        } else {
            HighlightTheme::default_light()
        };
    }
    // A theme change can reset fonts, so put the chosen ones back.
    ui::themes::apply_font(cx);
}

/// Passes backend events (data changes, prompts, stopped forwards) to the UI.
fn pump_events(mut events: mpsc::UnboundedReceiver<BackendEvent>, window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            let _ = cx.update(|cx| {
                let store = Store::global(cx);
                match event {
                    BackendEvent::DataChanged => {
                        store.update(cx, |store, cx| store.refresh(cx));
                        look::pull(cx);
                    }
                    BackendEvent::SyncStatus(state) => store.update(cx, |store, cx| {
                        store.sync = state.into();
                        cx.notify();
                    }),
                    BackendEvent::ForwardStopped { rule_id, reason } => {
                        store.update(cx, |store, cx| store.refresh_forwards(cx));
                        let label = store
                            .read(cx)
                            .forwards
                            .iter()
                            .find(|f| f.id == rule_id)
                            .map(|f| f.label.clone())
                            .unwrap_or_else(|| "Port forward".into());
                        // It can stop while the user is elsewhere, so tell the notification center.
                        let reason = reason.unwrap_or_else(|| "It stopped.".into());
                        ui::native::notify(&format!("{label} stopped"), &reason, cx);
                    }
                    BackendEvent::Prompt(request) => {
                        let _ = window.update(cx, |_, window, cx| ui::prompts::show(request, window, cx));
                    }
                }
            });
        }
    })
    .detach();
}

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        // Agents start the app this way. It only passes messages to the running
        // app, so it must print nothing else.
        Some("--mcp") => std::process::exit(mcp::run_bridge()),
        // Keeps agents working while the app is closed.
        Some(mcp::SERVICE_ARG) => std::process::exit(mcp::run_service()),
        Some(mcp::WORKER_ARG) => std::process::exit(mcp::run_worker()),
        _ => {}
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    runtime::init()?;
    mcp::claim();
    let (events_tx, events_rx) = mpsc::unbounded();
    let services = Arc::new(runtime::block_on(Services::build(events_tx))?);

    gpui_kit::application().with_assets(host_os::Assets).run(move |cx| {
        // Windows and Linux name the app in notifications by this.
        cx.set_app_identity("app.somme.desktop", ui::common::APP_NAME);
        gpui_kit::init(cx);
        ui::themes::register(cx);
        terminal::view::init(cx);
        editor::pane::init(cx);
        ui::workspace::init(cx);
        ui::menus::init(cx);
        cx.set_global(Settings::load(&services.data_dir));
        cx.set_global(SettingsFile(services.data_dir.clone()));
        apply_appearance(cx);
        ui::themes::apply_font(cx);
        cx.observe_global::<Settings>(ui::themes::apply_font).detach();
        Store::init(services.clone(), cx);
        host_os::init(&services.data_dir, cx);
        look::init(services.clone(), cx);
        mcp::init(services.clone(), cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1200.), px(780.)), cx)),
            window_min_size: Some(size(px(720.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some(ui::common::APP_NAME.into()),
                ..TitleBar::title_bar_options()
            }),
            ..TitleBar::window_options()
        };
        let quit_services = services.clone();
        let (window, _workspace) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Workspace::new(services, window, cx))
        })
        .expect("open window");
        pump_events(events_rx, window, cx);
        cx.on_app_quit(move |_| {
            let services = quit_services.clone();
            async move { runtime::io(async move { services.shutdown().await }).await }
        })
        .detach();
        #[cfg(feature = "snapshot")]
        automation::start(window, cx);
        cx.activate(true);
    });
    Ok(())
}
