#[cfg(feature = "snapshot")]
mod automation;
mod runtime;
mod services;
mod settings;
mod sftp;
mod store;
mod terminal;
mod ui;

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

use crate::services::{BackendEvent, Services};
use crate::settings::{Appearance, Settings, SettingsFile};
use crate::store::Store;
use crate::ui::workspace::Workspace;

pub(crate) fn apply_appearance(cx: &mut App) {
    match Settings::get(cx).appearance {
        Appearance::Dark => Theme::change(ThemeMode::Dark, None, cx),
        Appearance::Light => Theme::change(ThemeMode::Light, None, cx),
        Appearance::System => Theme::sync_system_appearance(None, cx),
    }
}

/// Passes backend events (data changes, prompts, stopped forwards) to the UI.
fn pump_events(mut events: mpsc::UnboundedReceiver<BackendEvent>, window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            let _ = cx.update(|cx| {
                let store = Store::global(cx);
                match event {
                    BackendEvent::DataChanged => store.update(cx, |store, cx| store.refresh(cx)),
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
                        let _ = window.update(cx, |_, window, cx| {
                            let reason = reason.unwrap_or_else(|| "It stopped.".into());
                            ui::common::notify_error(window, cx, &format!("{label} stopped"), reason);
                        });
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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    runtime::init()?;
    let (events_tx, events_rx) = mpsc::unbounded();
    let services = Arc::new(runtime::block_on(Services::build(events_tx))?);

    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        terminal::view::init(cx);
        ui::workspace::init(cx);
        cx.set_global(Settings::load(&services.data_dir));
        cx.set_global(SettingsFile(services.data_dir.clone()));
        apply_appearance(cx);
        Store::init(services.clone(), cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1200.), px(780.)), cx)),
            window_min_size: Some(size(px(720.), px(480.))),
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
