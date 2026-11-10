mod analysis;
mod commands;
mod error;
mod autotag;
mod import;
mod ingest;
mod jobs;
mod library;
mod menu;
mod persistence;
mod playlist;
mod rekordbox;
mod rename;
mod secrets;
mod slskd;
mod sources;
mod state;
mod text;

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;
use tracing::{error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use import::LibraryStore;
use jobs::JobStore;
use slskd::Daemon;
use sources::Sources;
use state::SettingsState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    rename::move_data_dirs(&context.config().identifier);

    let app = tauri::Builder::default()
        // A second copy would start its own slskd and fight over the Soulseek
        // login, so focus the existing window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| menu::show_main(app)))
        // Remembers the window's size and position. Not its visibility: the
        // window may have been closed (hidden) when the app quit.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .menu(menu::build)
        .on_menu_event(menu::handle_event)
        // Closing the window hides it, as macOS apps that work in the background
        // do: downloads and Soulseek sharing carry on, and clicking the Dock
        // icon brings it back. Quitting (⌘Q) stops everything.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && cfg!(target_os = "macos") {
                    api.prevent_close();
                    hide_window(window);
                }
            }
        })
        .setup(|app| {
            init_logging(app.handle());
            let settings = persistence::load_settings(app.handle())?;
            app.manage(SettingsState(std::sync::Mutex::new(settings)));
            app.manage(JobStore::load(app.handle()));
            app.manage(LibraryStore::new());
            app.manage(ingest::ArrivalQueue::load(app.handle()));
            app.manage(analysis::AnalysisQueue::default());
            app.manage(commands::collection::AudioStream::default());
            app.manage(playlist::PlaylistStore::load(app.handle()));
            let backups = app.path().app_data_dir()?.join("rekordbox-backups");
            app.manage(rekordbox::Collection::new(backups));
            app.manage(Sources::new());
            app.manage(Daemon::new());

            tauri::async_runtime::spawn(ingest::retry_when_rekordbox_closes(app.handle().clone()));

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = handle.state::<Daemon>().start(&handle).await {
                    error!("Failed to start slskd: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::set_soulseek_credentials,
            commands::daemon::get_daemon_status,
            commands::daemon::restart_daemon,
            commands::search::search_tracks,
            commands::stores::find_buy_links,
            commands::jobs::list_jobs,
            commands::jobs::start_job,
            commands::jobs::retry_job,
            commands::jobs::retry_not_found_jobs,
            commands::jobs::redownload_job,
            commands::jobs::cancel_job,
            commands::jobs::download_candidate,
            commands::jobs::download_from_youtube,
            commands::jobs::youtube_tool,
            commands::jobs::remove_job,
            commands::library::get_library,
            commands::library::identify_tracks,
            commands::library::library_cover_art,
            commands::library::redownload_track,
            commands::library::apply_library_matches,
            commands::library::cancel_library_task,
            commands::library::dismiss_library_items,
            commands::library::change_music_dir,
            commands::collection::open_collection,
            commands::collection::collection_state,
            commands::collection::collection_tracks,
            commands::collection::collection_track_ids,
            commands::collection::find_in_collection,
            commands::collection::edit_collection,
            commands::collection::import_to_collection,
            commands::collection::analyse_tracks,
            commands::collection::cancel_analysis,
            commands::collection::analysis_progress,
            commands::collection::track_preparation,
            commands::collection::track_waveform,
            commands::collection::track_audio,
            commands::collection::stop_track_audio,
            commands::playlist::import_spotify_playlist,
            commands::playlist::linked_playlist,
            commands::playlist::set_playlist_match,
        ])
        .build(context)
        .expect("error while building tauri application");

    app.run(|handle, event| match event {
        RunEvent::Exit => {
            tauri::async_runtime::block_on(handle.state::<Daemon>().stop(handle));
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { has_visible_windows: false, .. } => menu::show_main(handle),
        _ => {}
    });
}

/// Hides the window, leaving full screen first: hiding a full-screen window
/// would leave its empty Space behind.
fn hide_window(window: &tauri::Window) {
    if window.is_fullscreen().unwrap_or(false) {
        let _ = window.set_fullscreen(false);
        let window = window.clone();
        tauri::async_runtime::spawn(async move {
            // Let the exit animation finish first.
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let _ = window.hide();
        });
    } else {
        let _ = window.hide();
    }
}

/// The log file is started afresh once it passes this size; the previous one is kept.
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Logs to the console and to `app.log` in the app's log folder
/// (~/Library/Logs/<identifier> on macOS), which includes slskd's output.
fn init_logging(app: &AppHandle) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,slskd=info"));
    let log_dir = app.path().app_log_dir().ok();
    let file = log_dir.as_deref().and_then(|dir| open_log_file(dir).ok());
    let file_layer = file.map(|f| fmt::layer().with_ansi(false).with_writer(Mutex::new(f)));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(file_layer)
        .init();
    if let Some(dir) = log_dir {
        info!("Logging to {}", dir.join("app.log").display());
    }
}

fn open_log_file(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("app.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        std::fs::rename(&path, dir.join("app.previous.log"))?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}
