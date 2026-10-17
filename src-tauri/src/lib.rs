mod commands;
mod error;
mod events;
mod state;

use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,russh=warn")))
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(AppState::build(&handle))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| {
            tracing::error!(%err, "failed to start Somme");
            std::process::exit(1);
        });

    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>() {
                tauri::async_runtime::block_on(state.shutdown());
            }
    });
}
