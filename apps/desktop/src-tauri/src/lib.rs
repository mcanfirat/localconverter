//! LocalConvert desktop shell.
//!
//! Thin by design: it owns the window, the IPC surface and the job registry.
//! Every decision about files, validation and safety lives in
//! `localconvert-core`, where it can be tested without a GUI.

mod commands;
pub mod state;

use std::path::PathBuf;

use tauri::Manager;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_operations,
            commands::preflight_images,
            commands::media_available,
            commands::list_jobs,
            commands::get_job,
            commands::start_job,
            commands::cancel_job,
            commands::clear_completed_jobs,
        ])
        .setup(|app| {
            let temp_root = app
                .path()
                .temp_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("localconvert");
            std::fs::create_dir_all(&temp_root)?;

            init_logging(app.path().app_log_dir().ok());
            tracing::info!(
                version = localconvert_core::VERSION,
                temp_root = ?temp_root,
                "LocalConvert starting"
            );

            let state = AppState::new(temp_root.clone());

            // Startup recovery: a previous crash may have left job workspaces
            // behind. Nothing is running yet, so every one of them is stale.
            match localconvert_core::cleanup_stale(&temp_root, &state.active_job_ids()) {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "removed stale job workspaces"),
                Err(err) => tracing::warn!(error = %err, "startup cleanup failed"),
            }

            app.manage(state);
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(err) = result {
        eprintln!("LocalConvert failed to start: {err}");
        std::process::exit(1);
    }
}

/// Logs go to stderr and, when a log directory is available, to a rolling local
/// file. Nothing is ever sent off the machine.
fn init_logging(log_dir: Option<PathBuf>) {
    let filter = EnvFilter::try_from_env("LOCALCONVERT_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,localconvert_core=debug"));

    let file_layer = log_dir.and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(tracing_appender::rolling::daily(dir, "localconvert.log")),
        )
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .try_init();
}
