//! Writing playlists to a USB stick for players (CDJs and the like), through
//! rbxport's `rbl-export`: the audio, its analysis, and both of the
//! databases a player may read (`export.pdb` and `exportLibrary.db`).
//!
//! A stick is made to hold exactly the playlists chosen. One written here
//! before is brought up to date rather than written again: only what
//! changed is copied, and tracks no longer chosen come off.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rbl_export::cache::PreparationCache;
use rbl_export::{CompatibilityFormat, ExportError, ExportOptions, ExportRoot};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::rekordbox::{Collection, ExportSelection};

/// How much converted audio is kept between exports, so the next doesn't convert it again.
const CACHE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The export under way, if one is, and what is kept between them.
#[derive(Default)]
pub struct Exports {
    /// The stick being written, and the flag that stops it.
    running: Mutex<Option<(PathBuf, Arc<AtomicBool>)>>,
    cache: OnceLock<PreparationCache>,
}

impl Exports {
    fn cache(&self, app: &AppHandle) -> &PreparationCache {
        self.cache.get_or_init(|| {
            let dir = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("jim-box"));
            PreparationCache::new(dir.join("usb-export"))
        })
    }

    /// Asks the export under way to stop. It stops between files, and the
    /// stick is left as it was before.
    pub fn cancel(&self) {
        if let Some((_, cancel)) = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Lets go of the stick when the export is over, however it ends.
struct Running<'a>(&'a Exports);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.0.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// A volume an export could be written to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    pub path: PathBuf,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub file_system: String,
    /// What it already holds for players, if anything.
    pub holds: Option<Holds>,
    /// The library's playlists it was last written with, when it was
    /// written from this library: what to offer again.
    pub playlist_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holds {
    pub tracks: usize,
    pub playlists: usize,
    /// Written by this app (or rbxport), so it can be brought up to date;
    /// one rekordbox wrote is written in full.
    pub ours: bool,
}

/// The volumes worth offering, with what each holds. Reads each stick.
pub fn devices(app: &AppHandle) -> Vec<Device> {
    let db_id = app.state::<Collection>().library_db_id().unwrap_or(0);
    rbl_devices::list()
        .into_iter()
        .map(|device| {
            let holds = rbl_devices::inspect(&device.mount_point)
                .map(|found| Holds { tracks: found.tracks, playlists: found.playlists, ours: found.ours });
            let playlist_ids = rbl_export::Manifest::load(&device.mount_point)
                .filter(|manifest| db_id != 0 && manifest.db_id == db_id)
                .map(|manifest| {
                    manifest
                        .playlists
                        .iter()
                        .filter(|p| !p.folder && !p.device_only && p.library_id != 0)
                        .map(|p| p.library_id.to_string())
                        .collect()
                })
                .unwrap_or_default();
            Device {
                name: device.name,
                path: device.mount_point,
                total_bytes: device.total_bytes,
                free_bytes: device.free_bytes,
                file_system: device.file_system,
                holds,
                playlist_ids,
            }
        })
        .collect()
}

/// What an export would do to a stick, worked out without writing to it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub tracks: usize,
    pub tracks_new: usize,
    pub tracks_changed: usize,
    pub tracks_kept: usize,
    pub tracks_removed: usize,
    /// Chosen tracks whose file can't be found; they'd be left out.
    pub tracks_missing: usize,
    /// Bytes of audio to copy.
    pub copy_bytes: u64,
    /// The room the export needs while it's written.
    pub needed_bytes: u64,
    /// The room on the stick now; None when it can't be told.
    pub free_bytes: Option<u64>,
    /// Converted files' sizes aren't known until they're converted.
    pub approximate: bool,
}

/// Which folder a stick with no export yet gets: hidden on a Mac-formatted
/// one, as rekordbox does it.
fn preferred_root(destination: &Path) -> (Option<ExportRoot>, Option<u64>) {
    let device = rbl_devices::list().into_iter().find(|device| device.mount_point == destination);
    let free = device.as_ref().filter(|d| d.total_bytes > 0).map(|d| d.free_bytes);
    (device.and_then(|d| ExportRoot::for_file_system(&d.file_system)), free)
}

fn options<'a>(
    selection: &'a ExportSelection,
    convert: Option<CompatibilityFormat>,
    root: Option<ExportRoot>,
    cache: &'a PreparationCache,
) -> ExportOptions<'a> {
    ExportOptions { sync: Some(&selection.sync), compatibility: convert, root, cache: Some(cache), ..Default::default() }
}

fn usb_err(e: &ExportError) -> AppError {
    match e {
        ExportError::Io(io) if io.kind() == std::io::ErrorKind::StorageFull => {
            AppError::Usb(format!("There isn't enough room on the stick ({io})"))
        }
        ExportError::DeviceGone => AppError::Usb("The stick was unplugged before the export finished. Plug it back in and export again; it carries on from there.".into()),
        ExportError::Empty => AppError::Usb("Those playlists have no tracks to export".into()),
        _ => AppError::Usb(e.to_string()),
    }
}

fn check_ready(destination: &Path, playlists: &[String]) -> AppResult<()> {
    if playlists.is_empty() {
        return Err(AppError::Usb("Choose the playlists to put on the stick".into()));
    }
    if !destination.is_dir() {
        return Err(AppError::Usb("That stick is no longer connected".into()));
    }
    Ok(())
}

/// What exporting `playlists` to the stick at `destination` would copy and
/// need. Blocking.
pub fn estimate(
    app: &AppHandle,
    destination: &Path,
    playlists: &[String],
    convert: Option<CompatibilityFormat>,
) -> AppResult<Estimate> {
    check_ready(destination, playlists)?;
    let selection = app.state::<Collection>().export_selection(playlists)?;
    let (root, free_bytes) = preferred_root(destination);
    let exports = app.state::<Exports>();
    let plan = rbl_export::plan::preview(
        destination,
        &selection.tracks,
        &selection.playlists,
        &selection.my_tags,
        &options(&selection, convert, root, exports.cache(app)),
    )
    .map_err(|e| usb_err(&e))?;
    let found = plan.estimate;
    Ok(Estimate {
        tracks: selection.tracks.len(),
        tracks_new: found.tracks_new,
        tracks_changed: found.tracks_changed,
        tracks_kept: found.tracks_kept,
        tracks_removed: found.tracks_removed,
        tracks_missing: found.tracks_missing,
        copy_bytes: found.copy_bytes,
        needed_bytes: plan.peak_bytes,
        free_bytes,
        approximate: plan.approximate || found.approximate || plan.unknown_size,
    })
}

/// Where an export has got to; sent as `usb-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub path: PathBuf,
    /// `preparing`, `checking`, `copying`, `converting`, `database`,
    /// `verifying`, `publishing` or `ejecting`.
    pub stage: &'static str,
    /// Tracks dealt with in this stage, and how many it has.
    pub done: usize,
    pub total: usize,
    /// The track being dealt with.
    pub title: String,
    /// How far through that track's file, where it's being copied.
    pub file_done: u64,
    pub file_total: u64,
    /// Whether it can still be stopped: not once the stick is being changed over.
    pub can_cancel: bool,
}

/// How an export went.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub path: PathBuf,
    /// The stick already held exactly this, and nothing was written.
    pub up_to_date: bool,
    pub tracks: usize,
    pub playlists: usize,
    pub tracks_added: usize,
    pub tracks_updated: usize,
    pub removed: usize,
    pub converted: usize,
    pub bytes_copied: u64,
    /// "title: reason" for tracks left out: their file was missing or couldn't be read.
    pub left_out: Vec<String>,
    pub ejected: bool,
    /// Why it couldn't be ejected, when that was asked for.
    pub eject_error: Option<String>,
}

/// Writes `playlists` to the stick at `destination`, saying how it goes as
/// `usb-progress` events, and ejects it afterwards if asked. Blocking, for
/// as long as the copying takes.
pub fn export(
    app: &AppHandle,
    destination: &Path,
    playlists: &[String],
    convert: Option<CompatibilityFormat>,
    eject: bool,
) -> AppResult<Report> {
    check_ready(destination, playlists)?;
    // rekordbox rewrites a stick's databases when it sees one.
    if rbl_db::is_rekordbox_app_running() {
        return Err(AppError::Usb("Quit rekordbox before exporting, so only one app writes to the stick".into()));
    }
    let exports = app.state::<Exports>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = exports.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if running.is_some() {
            return Err(AppError::Usb("An export is already under way".into()));
        }
        *running = Some((destination.to_owned(), Arc::clone(&cancel)));
    }
    let _running = Running(&exports);

    let last = std::cell::Cell::new((Instant::now(), ""));
    let emit = |progress: Progress| {
        let (at, stage) = last.get();
        // Every change of stage, and the rest a few times a second.
        if stage == progress.stage && at.elapsed() < Duration::from_millis(100) {
            return;
        }
        last.set((Instant::now(), progress.stage));
        let _ = app.emit("usb-progress", progress);
    };
    let stage = |stage: &'static str| Progress {
        path: destination.to_owned(),
        stage,
        done: 0,
        total: 0,
        title: String::new(),
        file_done: 0,
        file_total: 0,
        can_cancel: stage == "preparing",
    };
    emit(stage("preparing"));

    // The stick it is now: one swapped for another under the same name is refused.
    let volume = rbl_core::durable::VolumeIdentity::capture(destination)
        .map_err(|_| AppError::Usb("That stick is no longer connected".into()))?;
    // An export cut short (the stick pulled) is put right first.
    rbl_export::recover(destination).map_err(|e| AppError::Usb(format!("The stick couldn't be read: {e}")))?;
    let selection = app.state::<Collection>().export_selection(playlists)?;
    let (root, _) = preferred_root(destination);
    let cache = exports.cache(app);

    let result = rbl_export::export_cancellable(
        destination,
        &selection.tracks,
        &selection.playlists,
        &selection.my_tags,
        &ExportOptions { expected_volume: Some(&volume), ..options(&selection, convert, root, cache) },
        &mut |p| {
            let per_file = p.unit != "tracks" && p.tracks_total > 0;
            emit(Progress {
                path: destination.to_owned(),
                stage: p.stage,
                done: if per_file { p.tracks_done } else { p.done },
                total: if per_file { p.tracks_total } else { p.total },
                title: p.title.clone(),
                file_done: if p.unit == "bytes" { p.bytes_done } else { 0 },
                file_total: if p.unit == "bytes" { p.bytes_total } else { 0 },
                can_cancel: !matches!(p.stage, "verifying" | "publishing"),
            });
        },
        &|| cancel.load(Ordering::Relaxed),
    );
    if let Err(e) = cache.prune(CACHE_BYTES) {
        tracing::warn!("Couldn't tidy the USB export cache: {e}");
    }
    let report = result.map_err(|e| match e {
        // Pulled out part way: the error itself is whatever write failed first.
        ExportError::Io(_) if !destination.is_dir() => usb_err(&ExportError::DeviceGone),
        e => usb_err(&e),
    })?;
    if !report.verified {
        return Err(AppError::Usb("The export was written but didn't read back right. Export again.".into()));
    }

    let (ejected, eject_error) = if eject {
        emit(stage("ejecting"));
        match rbl_devices::eject::eject(destination) {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        }
    } else {
        (false, None)
    };
    let mut left_out = report.skipped;
    left_out.extend(report.failed);
    left_out.extend(report.warnings.into_iter().map(|w| format!("{}: {}", w.title, w.reason)));
    Ok(Report {
        path: destination.to_owned(),
        up_to_date: report.up_to_date,
        tracks: report.tracks,
        playlists: report.playlists,
        tracks_added: report.tracks_added,
        tracks_updated: report.tracks_updated,
        removed: report.removed,
        converted: report.converted,
        bytes_copied: report.bytes_copied,
        left_out,
        ejected,
        eject_error,
    })
}

/// Ejects a stick, so it's safe to pull out. Refused while it's being written.
pub fn eject(app: &AppHandle, path: &Path) -> AppResult<()> {
    let exports = app.state::<Exports>();
    let busy = exports
        .running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|(writing, _)| writing == path);
    if busy {
        return Err(AppError::Usb("That stick is being written to".into()));
    }
    rbl_devices::eject::eject(path).map_err(|e| AppError::Usb(format!("The stick couldn't be ejected: {e}")))
}
