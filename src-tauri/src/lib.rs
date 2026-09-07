//! The Relay shell: transport only. Engine decisions belong in `relay-core`.

mod commands;
mod state;

use tauri::Manager;

use crate::state::AppState;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RELAY_LOG")
                .unwrap_or_else(|_| "relay_core=info,relay_app=info".into()),
        )
        .init();

    // Phase 5 requires a crash dialog and a log path rather than a silent exit, so the
    // shell must not abort on panic. The hook is installed before any window exists.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic in the shell: {info}");
        default_hook(info);
    }));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(AppState::start(app.handle()));

            // The window is created hidden so the first paint is themed rather than a
            // white flash; the frontend reveals it once tokens are applied.
            if let Some(window) = app.get_webview_window("main") {
                window.show()?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::servers_list,
            commands::servers_save,
            commands::servers_delete,
            commands::session_open,
            commands::session_test,
            commands::secrets_status,
            commands::secrets_set,
            commands::secrets_clear,
            commands::session_close,
            commands::session_list_dir,
            commands::session_mkdir,
            commands::session_rename,
            commands::session_remove,
            commands::session_logs,
            commands::local_list_dir,
            commands::local_default_dir,
            commands::local_roots,
            commands::queue_enqueue,
            commands::queue_control,
            commands::resolve_prompt,
            commands::settings_get,
            commands::settings_set,
            commands::engine_subscribe,
            commands::engine_unsubscribe,
            commands::engine_snapshot,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Relay window")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Give sessions and the pump their cooperative shutdown before exit.
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(state.shutdown());
            }
        });
}
