use tauri::{AppHandle, Manager, State};
use tracing::error;

use crate::error::{AppError, AppResult};
use crate::library::validate_template;
use crate::slskd::Daemon;
use crate::sources::youtube;
use crate::state::{Settings, SettingsState};
use crate::{persistence, secrets};

#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsState>) -> Settings {
    settings.snapshot()
}

/// Saves format/sharing/path settings. The music folder and Soulseek
/// username have dedicated commands (`change_music_dir`,
/// `set_soulseek_credentials`) and are ignored here.
#[tauri::command]
pub async fn update_settings(app: AppHandle, settings: Settings) -> AppResult<Settings> {
    validate(&settings)?;

    let state = app.state::<SettingsState>();
    let (previous, updated) = {
        let mut current = state.0.lock().expect("settings lock poisoned");
        let previous = current.clone();
        *current = Settings {
            music_dir: previous.music_dir.clone(),
            soulseek_username: previous.soulseek_username.clone(),
            ..settings
        };
        (previous, current.clone())
    };
    persistence::save_settings(&app, &updated);

    if previous.daemon_config_differs(&updated) {
        restart_daemon_in_background(app);
    }
    Ok(updated)
}

/// Stores the password in the keychain and (re)connects to Soulseek.
#[tauri::command]
pub async fn set_soulseek_credentials(app: AppHandle, username: String, password: String) -> AppResult<()> {
    let username = username.trim().to_string();
    if username.is_empty() || password.is_empty() {
        return Err(AppError::Validation("Enter both a username and a password".into()));
    }

    secrets::set_password(&username, &password)?;

    let state = app.state::<SettingsState>();
    let (previous_username, updated) = {
        let mut current = state.0.lock().expect("settings lock poisoned");
        let previous = current.soulseek_username.replace(username.clone());
        (previous, current.clone())
    };
    persistence::save_settings(&app, &updated);

    if let Some(old) = previous_username.filter(|old| *old != username) {
        if let Err(e) = secrets::delete_password(&old) {
            error!("Failed to remove old keychain entry for {old}: {e}");
        }
    }

    app.state::<Daemon>().start(&app).await
}

fn validate(settings: &Settings) -> AppResult<()> {
    if settings.formats.is_empty() {
        return Err(AppError::Validation("Allow at least one audio format".into()));
    }
    if let Some(browser) = &settings.youtube_cookies_browser {
        if !youtube::COOKIE_BROWSERS.contains(&browser.as_str()) {
            return Err(AppError::Validation(format!("yt-dlp can't read cookies from “{browser}”")));
        }
    }
    let paths = &settings.paths;
    [&paths.default, &paths.comp, &paths.singleton].into_iter().try_for_each(|t| validate_template(t))
}

fn restart_daemon_in_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = app.state::<Daemon>().start(&app).await {
            error!("Failed to restart slskd: {e}");
        }
    });
}
