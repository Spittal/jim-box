//! Bringing the collection together: the files of collection tracks are
//! moved from the places the user picks into one library folder, filed by
//! the path templates, and keep their place in rekordbox's collection.
//!
//! Files on an exported rekordbox drive are left where they are. The drive's
//! own libraries (`export.pdb`, `exportLibrary.db`) point at them by path,
//! and nothing here rewrites those, so moving one would break the drive.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use super::{LibraryStore, RelocateReport};
use crate::error::{AppError, AppResult};
use crate::ingest;
use crate::jobs::JobStore;
use crate::library;
use crate::persistence;
use crate::rekordbox::{Arrival, Collection};
use crate::slskd::Daemon;
use crate::state::{LibraryPhase, PathTemplates, Settings, SettingsState};

/// How much of a path tells one place from another when files are outside
/// every library folder: `/Volumes/Drive/Folder`, `/Users/name/Music`,
/// `C:\Users\name\Music`. Each such group is then named by the deepest
/// folder that holds all its files.
const PLACE_COMPONENTS: usize = if cfg!(windows) { 5 } else { 4 };
/// How many emptied folders above a moved file are removed.
const TIDY_LEVELS: usize = 2;

/// Somewhere collection tracks are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub path: PathBuf,
    /// Tracks whose files are here and could be moved.
    pub tracks: usize,
    /// The size of those files.
    pub bytes: u64,
    /// One of the library folders, rather than somewhere rekordbox has
    /// tracks that isn't one (yet).
    pub library: bool,
}

/// Where the collection's files are, for choosing what to consolidate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidatePlan {
    /// The library folders, primary first, then the other places, most
    /// tracks first.
    pub places: Vec<Place>,
    /// Tracks whose file can't be found. Left alone.
    pub missing: usize,
    /// Tracks whose file is on an exported rekordbox drive. Left alone.
    pub exported: usize,
    pub rekordbox_running: bool,
}

/// The collection's files, sorted out by where they are.
#[derive(Debug, Default)]
struct Survey {
    /// Each place with its files, in the order of [`ConsolidatePlan::places`].
    places: Vec<(Place, Vec<PathBuf>)>,
    missing: usize,
    exported: usize,
}

fn survey(files: Vec<PathBuf>, settings: &Settings) -> Survey {
    let mut survey = Survey::default();
    let mut in_library: HashMap<&Path, Vec<(PathBuf, u64)>> = HashMap::new();
    let mut elsewhere: HashMap<PathBuf, Vec<(PathBuf, u64)>> = HashMap::new();
    // Albums share a folder, so each folder is looked at once.
    let mut on_export: HashMap<PathBuf, bool> = HashMap::new();
    for file in files {
        let Some(size) = std::fs::metadata(&file).ok().filter(|meta| meta.is_file()).map(|meta| meta.len()) else {
            survey.missing += 1;
            continue;
        };
        let folder = file.parent().unwrap_or(Path::new("")).to_path_buf();
        if *on_export.entry(folder.clone()).or_insert_with_key(|folder| is_on_exported_drive(folder)) {
            survey.exported += 1;
            continue;
        }
        match settings.library_root_of(&file) {
            Some(root) => in_library.entry(root).or_default().push((file, size)),
            None => elsewhere.entry(folder.components().take(PLACE_COMPONENTS).collect()).or_default().push((file, size)),
        }
    }

    let place = |path: PathBuf, library: bool, mut files: Vec<(PathBuf, u64)>| {
        files.sort();
        let bytes = files.iter().map(|(_, size)| size).sum();
        let files: Vec<PathBuf> = files.into_iter().map(|(file, _)| file).collect();
        (Place { path, tracks: files.len(), bytes, library }, files)
    };
    for root in settings.library_roots() {
        survey.places.push(place(root.to_path_buf(), true, in_library.remove(root).unwrap_or_default()));
    }
    let mut others: Vec<_> =
        elsewhere.into_values().map(|files| place(common_folder(files.iter().map(|(file, _)| file.as_path())), false, files)).collect();
    others.sort_by(|(a, _), (b, _)| b.tracks.cmp(&a.tracks).then_with(|| a.path.cmp(&b.path)));
    survey.places.extend(others);
    survey
}

/// The deepest folder that holds every one of `files`.
fn common_folder<'a>(files: impl Iterator<Item = &'a Path>) -> PathBuf {
    let mut common: Option<PathBuf> = None;
    for file in files {
        let folder = file.parent().unwrap_or(Path::new(""));
        common = Some(match common {
            None => folder.to_path_buf(),
            Some(so_far) => so_far.components().zip(folder.components()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect(),
        });
    }
    common.unwrap_or_default()
}

/// Whether `folder` is in the `Contents` tree of a rekordbox export: a
/// `Contents` folder beside the `PIONEER` (or hidden `.PIONEER`) folder that
/// holds the drive's libraries. A drive that merely has a `PIONEER` folder,
/// as rekordbox gives any drive it has seen, doesn't count: an export keeps
/// its audio under `Contents`.
fn is_on_exported_drive(folder: &Path) -> bool {
    folder.ancestors().any(|dir| {
        let is_contents = dir.file_name().is_some_and(|name| name.eq_ignore_ascii_case("Contents"));
        is_contents
            && dir.parent().is_some_and(|root| {
                ["PIONEER", ".PIONEER"].iter().any(|pioneer| root.join(pioneer).join("rekordbox").is_dir())
            })
    })
}

/// Where the path templates file `file`, going by its own tags. A file with
/// no title in its tags is filed under its name.
fn destination(target: &Path, paths: &PathTemplates, file: &Path) -> PathBuf {
    let mut meta = library::read_file(file).map(|scanned| scanned.tags.as_metadata()).unwrap_or_default();
    if meta.title.trim().is_empty() {
        meta.title = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    }
    library::library_path(target, &library::render(paths, &meta), &library::extension(file))
}

/// Removes the folders a moved file left empty, a couple of levels up at
/// most (its album and artist folders, not whatever they were kept in), and
/// never the place itself.
fn tidy(file: &Path, place: &Path) {
    let mut dir = file.parent();
    for _ in 0..TIDY_LEVELS {
        match dir {
            Some(folder) if folder != place && library::remove_if_empty(folder) => dir = folder.parent(),
            _ => break,
        }
    }
}

/// Looks at where the collection's files are, without moving anything.
pub async fn consolidate_plan(app: &AppHandle) -> AppResult<ConsolidatePlan> {
    let settings = app.state::<SettingsState>().snapshot();
    let app = app.clone();
    library::blocking(move || {
        let found = survey(app.state::<Collection>().track_files(), &settings);
        Ok(ConsolidatePlan {
            places: found.places.into_iter().map(|(place, _)| place).collect(),
            missing: found.missing,
            exported: found.exported,
            rekordbox_running: rbl_db::is_rekordbox_running(),
        })
    })
    .await
}

/// Moves the collection's files that are in the `sources` places (as
/// [`consolidate_plan`] names them) into the library folder `target`, never
/// overwriting, and points the tracks at their new files. A library folder
/// that all its tracks were moved out of stops being one.
pub async fn consolidate(app: &AppHandle, sources: Vec<PathBuf>, target: PathBuf) -> AppResult<RelocateReport> {
    let library = app.state::<LibraryStore>();
    if app.state::<JobStore>().has_running() || library.is_busy() || library.snapshot().progress.phase != LibraryPhase::Idle {
        return Err(AppError::Validation(
            "Wait for downloads and library work to finish before consolidating".into(),
        ));
    }
    // A moved file would show as missing in rekordbox until its track could
    // be told where it went.
    if rbl_db::is_rekordbox_running() {
        return Err(AppError::Validation(
            "Quit rekordbox first (and its agent in the menu bar): its library can't be updated while it's open".into(),
        ));
    }
    let settings = app.state::<SettingsState>().snapshot();
    if !settings.library_roots().any(|root| root == target) {
        return Err(AppError::Validation("Consolidate into one of the library folders".into()));
    }
    if !target.is_dir() {
        return Err(AppError::Validation(format!(
            "The folder {} isn't there. Is its drive connected?",
            target.display()
        )));
    }

    let (mover, into) = (app.clone(), target.clone());
    let result = library::blocking(move || Ok(move_in(&mover, &settings, &sources, &into))).await;
    // Whatever moved is on the queue: tell the collection even after a failure.
    ingest::flush(app).await;
    library.set_progress(app, LibraryPhase::Idle, 0, 0);
    let (report, emptied) = result?;
    if report.moved > 0 {
        // What was found for these tracks names their old paths.
        library.clear(app);
    }
    forget_library_dirs(app, &emptied, &target);
    info!("Consolidated the library: {} files moved, {} failed", report.moved, report.failed.len());
    Ok(report)
}

/// Takes library folders that were consolidated away off the list. Should
/// the primary be one of them, `target` becomes the primary. Soulseek stops
/// sharing them.
fn forget_library_dirs(app: &AppHandle, emptied: &[PathBuf], target: &Path) {
    if emptied.is_empty() {
        return;
    }
    let (previous, updated) = {
        let state = app.state::<SettingsState>();
        let mut settings = state.0.lock().expect("settings lock poisoned");
        let previous = settings.clone();
        forget(&mut settings, emptied, target);
        (previous, settings.clone())
    };
    persistence::save_settings(app, &updated);
    if previous.daemon_config_differs(&updated) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = app.state::<Daemon>().start(&app).await {
                warn!("Failed to restart slskd after consolidating: {e}");
            }
        });
    }
}

fn forget(settings: &mut Settings, emptied: &[PathBuf], target: &Path) {
    if emptied.contains(&settings.music_dir) && settings.library_dirs.iter().any(|dir| dir == target) {
        settings.music_dir = target.to_path_buf();
    }
    let primary = settings.music_dir.clone();
    settings.library_dirs.retain(|dir| *dir != primary && !emptied.contains(dir));
}

/// Moves the files, and says which library folders among the sources had
/// every one of their tracks moved out.
fn move_in(app: &AppHandle, settings: &Settings, sources: &[PathBuf], target: &Path) -> (RelocateReport, Vec<PathBuf>) {
    let store = app.state::<LibraryStore>();
    let places: Vec<(Place, Vec<PathBuf>)> = survey(app.state::<Collection>().track_files(), settings)
        .places
        .into_iter()
        .filter(|(place, _)| place.path != target && sources.contains(&place.path))
        .collect();
    let mut emptied: Vec<PathBuf> =
        places.iter().filter(|(place, _)| place.library).map(|(place, _)| place.path.clone()).collect();
    let files: Vec<(PathBuf, PathBuf)> = places
        .into_iter()
        .flat_map(|(place, files)| files.into_iter().map(move |file| (file, place.path.clone())))
        .collect();
    let mut report = RelocateReport { moved: 0, failed: Vec::new() };
    let total = files.len();
    for (done, (file, place)) in files.into_iter().enumerate() {
        store.set_progress(app, LibraryPhase::Moving, done, total);
        match library::move_file(&file, destination(target, &settings.paths, &file)) {
            Ok(dest) => {
                // Saved at once, so a track isn't lost track of should the
                // app stop part-way. The file is the same, so what rekordbox
                // has for its title, artist and the rest stays.
                let arrival = Arrival { path: dest, replaces: Some(file.clone()), moved_only: true, ..Default::default() };
                ingest::queue(app, vec![arrival]);
                tidy(&file, &place);
                report.moved += 1;
            }
            Err(e) => {
                // Still has a track in it: it stays a library folder.
                emptied.retain(|dir| *dir != place);
                report.failed.push(format!("{}: {e}", file.display()));
            }
        }
    }
    (report, emptied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "audio").unwrap();
    }

    fn settings(primary: &Path, others: &[&Path]) -> Settings {
        Settings { library_dirs: others.iter().map(|dir| dir.to_path_buf()).collect(), ..Settings::new(primary.to_path_buf()) }
    }

    #[test]
    fn leaves_exported_drives_and_missing_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        // An exported stick: audio under Contents, beside the libraries.
        let stick = dir.path().join("STICK");
        std::fs::create_dir_all(stick.join("PIONEER/rekordbox")).unwrap();
        let exported = stick.join("Contents/Artist/Album/a.flac");
        touch(&exported);
        // A drive rekordbox has merely seen: hidden folder, music elsewhere.
        let drive = dir.path().join("DRIVE");
        std::fs::create_dir_all(drive.join(".PIONEER/rekordbox")).unwrap();
        let loose = drive.join("Old Music/Artist/b.flac");
        touch(&loose);
        let gone = drive.join("Old Music/Artist/gone.flac");

        let found = survey(vec![exported, loose.clone(), gone], &settings(&dir.path().join("Library"), &[]));
        assert_eq!((found.exported, found.missing), (1, 1));
        let files: Vec<PathBuf> = found.places.into_iter().flat_map(|(_, files)| files).collect();
        assert_eq!(files, vec![loose]);
    }

    #[test]
    fn sorts_files_into_library_folders_and_other_places() {
        let dir = tempfile::tempdir().unwrap();
        let (primary, second) = (dir.path().join("Library"), dir.path().join("Second"));
        std::fs::create_dir_all(&primary).unwrap();
        let files = [
            second.join("Artist/Album/1.flac"),
            second.join("Artist/Album/2.flac"),
            dir.path().join("Stray/Deep/Artist/One/3.flac"),
            dir.path().join("Stray/Deep/Artist/Two/4.flac"),
        ];
        files.iter().for_each(|file| touch(file));

        let found = survey(files.to_vec(), &settings(&primary, &[&second]));
        let places: Vec<Place> = found.places.iter().map(|(place, _)| place.clone()).collect();
        // Other places are named by the deepest folder holding their files.
        let stray = dir.path().join("Stray/Deep/Artist");
        assert_eq!(
            places,
            vec![
                // A library folder is listed even with nothing in it.
                Place { path: primary, tracks: 0, bytes: 0, library: true },
                Place { path: second, tracks: 2, bytes: 10, library: true },
                Place { path: stray, tracks: 2, bytes: 10, library: false },
            ]
        );
    }

    #[test]
    fn consolidated_library_folders_come_off_the_list() {
        let (a, b, c) = (Path::new("/a"), Path::new("/b"), Path::new("/c"));
        let mut some = settings(a, &[b, c]);
        forget(&mut some, &[b.to_path_buf()], a);
        assert_eq!((some.music_dir.as_path(), &some.library_dirs), (a, &vec![c.to_path_buf()]));

        // The primary consolidated into another: that one becomes primary.
        let mut all = settings(a, &[b, c]);
        forget(&mut all, &[a.to_path_buf(), c.to_path_buf()], b);
        assert_eq!((all.music_dir.as_path(), all.library_dirs.is_empty()), (b, true));
    }

    #[test]
    fn names_a_place_by_the_deepest_folder_holding_its_files() {
        let files = ["/a/b/c/d/1.flac", "/a/b/c/e/f/2.flac", "/a/b/c/3.flac"].map(Path::new);
        assert_eq!(common_folder(files.into_iter()), PathBuf::from("/a/b/c"));
        assert_eq!(common_folder([Path::new("/a/b/1.flac")].into_iter()), PathBuf::from("/a/b"));
    }

    #[test]
    fn files_an_untagged_file_under_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Some Bootleg.mp3");
        touch(&file);
        assert_eq!(
            destination(Path::new("/m"), &PathTemplates::default(), &file),
            PathBuf::from("/m/Non-Album/Unknown Artist/Some Bootleg.mp3")
        );
    }

    #[test]
    fn tidies_only_the_folders_a_file_left_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Old/Artist/Album/a.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        tidy(&file, &dir.path().join("Old"));
        assert!(!dir.path().join("Old/Artist").exists());
        // Two levels at most: what held the artist folders stays.
        assert!(dir.path().join("Old").exists());

        // The place itself stays, even when it is left empty.
        let file = dir.path().join("Place/a.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        tidy(&file, &dir.path().join("Place"));
        assert!(dir.path().join("Place").exists());
    }

    /// Where the installed rekordbox library's files are, read from a copy;
    /// nothing is moved. Run with
    /// `MUSIC_DIR=/path cargo test live_plan -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_plan() {
        let music_dir = PathBuf::from(std::env::var("MUSIC_DIR").expect("MUSIC_DIR"));
        let installed = rbl_db::detect().expect("rekordbox library");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        let status = collection
            .load_from(rbl_db::LibraryLocation { master_db: copy, is_real_install: false, ..installed })
            .unwrap();
        let found = survey(collection.track_files(), &settings(&music_dir, &[]));
        println!("{} tracks; {} missing, {} on an exported drive", status.tracks, found.missing, found.exported);
        for (place, files) in &found.places {
            println!("  {} tracks, {} bytes, library {}: {}", place.tracks, place.bytes, place.library, place.path.display());
            if let Some(file) = files.first().filter(|_| !place.library) {
                println!("    {} -> {}", file.display(), destination(&music_dir, &PathTemplates::default(), file).display());
            }
        }
    }
}
