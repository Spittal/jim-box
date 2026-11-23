use std::path::PathBuf;

use rbl_export::CompatibilityFormat;
use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::library;
use crate::usb::{self, Device, Estimate, Exports, Report};

/// The sticks and drives plugged in, with what each already holds for players.
#[tauri::command]
pub async fn usb_devices(app: AppHandle) -> AppResult<Vec<Device>> {
    library::blocking(move || Ok(usb::devices(&app))).await
}

/// What exporting these playlists to a stick would copy, and the room it needs.
#[tauri::command]
pub async fn usb_estimate(
    app: AppHandle,
    path: PathBuf,
    playlists: Vec<String>,
    convert: Option<CompatibilityFormat>,
) -> AppResult<Estimate> {
    library::blocking(move || usb::estimate(&app, &path, &playlists, convert)).await
}

/// Makes a stick hold these playlists, for players to read. `convert` is the
/// format files older players can't read are converted to; None copies
/// every file as it is.
#[tauri::command]
pub async fn usb_export(
    app: AppHandle,
    path: PathBuf,
    playlists: Vec<String>,
    convert: Option<CompatibilityFormat>,
    eject: bool,
) -> AppResult<Report> {
    library::blocking(move || {
        let report = usb::export(&app, &path, &playlists, convert, eject);
        if !eject {
            crate::others::written(&app, &path);
        }
        report
    })
    .await
}

/// Stops the export under way; the stick is left as it was.
#[tauri::command]
pub fn cancel_usb_export(exports: State<'_, Exports>) {
    exports.cancel();
}

#[tauri::command]
pub async fn eject_usb(app: AppHandle, path: PathBuf) -> AppResult<()> {
    library::blocking(move || usb::eject(&app, &path)).await
}
