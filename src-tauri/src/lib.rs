mod analysis;
mod commands;
mod error;
mod autotag;
mod import;
mod ingest;
mod jobs;
mod libraries;
mod library;
mod menu;
mod persistence;
mod playlist;
mod process;
mod rekordbox;
mod rename;
mod secrets;
mod soulseek;
mod sources;
mod state;
mod text;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod tray;

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use soulseek::Soulseek;
use sources::Sources;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    rename::move_data_dirs(&context.config().identifier);

    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build);

    let app = builder
        // A second copy would log in too and fight over the Soulseek
        // session, so focus the existing window instead.
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
        .on_menu_event(menu::handle_event)
        // Closing the window hides it, as macOS apps that work in the background
        // do: downloads and Soulseek sharing carry on, and clicking the Dock
        // icon (the tray icon on Windows) brings it back. Quitting (⌘Q, or
        // Quit in the tray icon's menu) stops everything.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let can_reopen = cfg!(target_os = "macos") || tray::exists(window.app_handle());
                if window.label() == "main" && can_reopen {
                    api.prevent_close();
                    hide_window(window);
                }
            }
        })
        .setup(|app| {
            init_logging(app.handle());
            app.manage(analysis::AnalysisQueue::default());
            app.manage(commands::collection::AudioStream::default());
            app.manage(Sources::new());
            soulseek::raise_open_file_limit();
            app.manage(Soulseek::new());

            // Without it, closing the window quits the app instead of hiding it.
            #[cfg(not(target_os = "macos"))]
            if let Err(e) = tray::build(app.handle()) {
                tracing::warn!("Couldn't add the tray icon: {e}");
            }

            // With no library to open, the window asks for one, and the
            // rest is set up when it's chosen (see `libraries::open`).
            libraries::open_at_launch(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::libraries::library_choices,
            commands::libraries::open_library,
            commands::libraries::set_default_library,
            commands::libraries::forget_library,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::add_library_dir,
            commands::settings::remove_library_dir,
            commands::settings::set_primary_library_dir,
            commands::settings::set_soulseek_credentials,
            commands::soulseek::get_soulseek_status,
            commands::soulseek::reconnect_soulseek,
            commands::search::search_tracks,
            commands::search::track_preview,
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
            commands::library::library_cover_thumb,
            commands::library::redownload_track,
            commands::library::apply_library_matches,
            commands::library::cancel_library_task,
            commands::library::dismiss_library_items,
            commands::library::change_music_dir,
            commands::library::consolidate_plan,
            commands::library::consolidate_library,
            commands::collection::open_collection,
            commands::collection::collection_state,
            commands::collection::collection_tracks,
            commands::collection::collection_track_ids,
            commands::collection::find_in_collection,
            commands::collection::edit_collection,
            commands::collection::review_grids,
            commands::collection::import_to_collection,
            commands::collection::analyse_tracks,
            commands::collection::cancel_analysis,
            commands::collection::analysis_progress,
            commands::collection::track_preparation,
            commands::collection::track_waveform,
            commands::collection::track_previews,
            commands::collection::track_audio,
            commands::collection::stop_track_audio,
            commands::playlist::link_spotify_playlist,
            commands::playlist::linked_playlist,
            commands::playlist::followed_playlists,
            commands::playlist::set_playlist_settings,
            commands::playlist::unlink_playlist,
            commands::playlist::set_playlist_match,
        ])
        .build(context)
        .expect("error while building tauri application");

    app.run(|handle, event| match event {
        RunEvent::Exit => {
            tauri::async_runtime::block_on(handle.state::<Soulseek>().stop(handle));
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
/// (~/Library/Logs/<identifier> on macOS,
/// %LOCALAPPDATA%\<identifier>\logs on Windows).
fn init_logging(app: &AppHandle) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
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
