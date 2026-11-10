use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
use tracing::error;

use crate::error::AppResult;
use crate::library;
use crate::playlist::PlaylistMatch;
use crate::rekordbox::Arrival;
use crate::state::{Job, Settings};

const STORE_FILE: &str = "config.json";
const SETTINGS_KEY: &str = "settings";
const JOBS_KEY: &str = "jobs";
const PLAYLISTS_KEY: &str = "playlists";
const ARRIVALS_KEY: &str = "arrivals";

// --- Generic helpers ---

fn store_get<T: DeserializeOwned>(app: &AppHandle, key: &str) -> Option<T> {
    let store = app.store(STORE_FILE).ok()?;
    let value = store.get(key)?;
    match serde_json::from_value(value) {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Ignoring unreadable '{key}' in store: {e}");
            None
        }
    }
}

fn store_set<T: Serialize>(app: &AppHandle, key: &str, value: &T) {
    let store = match app.store(STORE_FILE) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {key}: {e}");
            return;
        }
    };
    match serde_json::to_value(value) {
        Ok(v) => store.set(key, v),
        Err(e) => {
            error!("Failed to serialize {key}: {e}");
            return;
        }
    }
    if let Err(e) = store.save() {
        error!("Failed to persist {key}: {e}");
    }
}

// --- Domain accessors ---

pub fn load_settings(app: &AppHandle) -> AppResult<Settings> {
    if let Some(mut settings) = store_get::<Settings>(app, SETTINGS_KEY) {
        if let Some(old) = settings.legacy_path_template.take() {
            settings.paths.default = library::convert_legacy_template(&old);
            save_settings(app, &settings);
        }
        return Ok(settings);
    }
    let music_dir = app.path().audio_dir()?.join("jim-box");
    Ok(Settings::new(music_dir))
}

pub fn save_settings(app: &AppHandle, settings: &Settings) {
    store_set(app, SETTINGS_KEY, settings);
}

pub fn load_jobs(app: &AppHandle) -> Vec<Job> {
    store_get(app, JOBS_KEY).unwrap_or_default()
}

pub fn save_jobs(app: &AppHandle, jobs: &[Job]) {
    store_set(app, JOBS_KEY, &jobs);
}

pub fn load_playlists(app: &AppHandle) -> Vec<PlaylistMatch> {
    store_get(app, PLAYLISTS_KEY).unwrap_or_default()
}

pub fn save_playlists(app: &AppHandle, playlists: &[PlaylistMatch]) {
    store_set(app, PLAYLISTS_KEY, &playlists);
}

pub fn load_arrivals(app: &AppHandle) -> Vec<Arrival> {
    store_get(app, ARRIVALS_KEY).unwrap_or_default()
}

pub fn save_arrivals(app: &AppHandle, arrivals: &[Arrival]) {
    store_set(app, ARRIVALS_KEY, &arrivals);
}
