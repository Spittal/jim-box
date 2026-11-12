//! Bringing files into the collection: importing new ones, and keeping a
//! track pointed at its file when that file is replaced, re-tagged or moved.
//! A track keeps its cues, grid and playlists through all of these, because
//! they belong to the track's row and not to the file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbl_db::import::{audio_files_in, is_audio, WalkBudget};
use rbl_db::write::{ImportOutcome, Writer, ROOT};
use serde::{Deserialize, Serialize};

use super::{rekordbox_err, Collection};
use crate::error::{AppError, AppResult};
use crate::text::{normalize, strip_noise};

/// How deep, and through how many entries, an import walks the folders it
/// is given: enough for any music folder, not for a whole disk.
const WALK_DEPTH: usize = 12;
const WALK_ENTRIES: usize = 200_000;

/// A file that has landed somewhere and belongs in the collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrival {
    pub path: PathBuf,
    /// Where the file was before, or the file it took the place of. The
    /// track that pointed there is pointed at `path` instead of making a
    /// new one.
    #[serde(default)]
    pub replaces: Option<PathBuf>,
    /// The same file, somewhere else: the track keeps the title, artist and
    /// the rest that rekordbox has for it, where a new or changed file has
    /// its tags read again.
    #[serde(default)]
    pub moved_only: bool,
    /// A top-level playlist to add the track to, by name, made if there isn't one.
    #[serde(default)]
    pub playlist: Option<String>,
    /// A playlist to add the track to, by id. Skipped if it has since been deleted.
    #[serde(default)]
    pub playlist_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: usize,
    /// Files that were in the collection already.
    pub existing: usize,
    /// "file: reason" for files that couldn't be read.
    pub skipped: Vec<String>,
    /// Every track the files belong to, new or not, in order.
    pub track_ids: Vec<String>,
}

impl Collection {
    /// Adds files, and the audio files in folders, to the collection, and to
    /// a playlist if one is given. Blocking.
    pub fn import(&self, paths: &[PathBuf], playlist_id: Option<&str>) -> AppResult<ImportReport> {
        let mut budget = WalkBudget::new(WALK_DEPTH, WALK_ENTRIES);
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(audio_files_in(path, &mut budget));
            } else if is_audio(path) {
                files.push(path.clone());
            }
        }
        if files.is_empty() {
            return Err(AppError::Validation("No music files found there".into()));
        }

        self.write(|writer, _| {
            let mut report = ImportReport::default();
            let outcomes = writer.import_files(&files).map_err(rekordbox_err)?;
            for (file, outcome) in files.iter().zip(outcomes) {
                match outcome {
                    ImportOutcome::Added(id) => {
                        report.added += 1;
                        report.track_ids.push(id);
                    }
                    ImportOutcome::Existing(id) => {
                        report.existing += 1;
                        report.track_ids.push(id);
                    }
                    ImportOutcome::Refused(reason) => {
                        let name = file.file_name().unwrap_or(file.as_os_str()).to_string_lossy();
                        report.skipped.push(format!("{name}: {reason}"));
                    }
                }
            }
            if let Some(playlist) = playlist_id {
                writer.add_tracks(playlist, &report.track_ids).map_err(rekordbox_err)?;
            }
            Ok(report)
        })
    }

    /// Takes arrived files into the collection in one go. Each gets its
    /// track's id, or why it couldn't be taken in; an error for the whole
    /// call (rekordbox is running) means none were. Blocking.
    pub fn take_in(&self, arrivals: &[Arrival]) -> AppResult<Vec<Result<String, String>>> {
        self.write(|writer, _| {
            let mut playlists = HashMap::new();
            Ok(arrivals
                .iter()
                .map(|arrival| take_in_one(writer, arrival, &mut playlists).map_err(|e| e.to_string()))
                .collect())
        })
    }

    /// Adds tracks to the top-level playlist called `name`, making it if
    /// there isn't one. Tracks already in it stay where they are; the rest
    /// go on the end in the order given. Returns the playlist's id. Blocking.
    #[cfg(test)]
    pub fn add_to_named_playlist(&self, name: &str, track_ids: &[String]) -> AppResult<String> {
        self.write(|writer, _| {
            let playlist = named_playlist(writer, name, &mut HashMap::new())?;
            writer.add_tracks(&playlist, track_ids).map_err(rekordbox_err)?;
            Ok(playlist)
        })
    }

    /// For each (title, artist), the track with that title and artist,
    /// compared loosely (case, accents and punctuation aside). Where no
    /// title is the same, one that differs only by noise is taken: "Song
    /// (feat. X)" for "Song", "Song - 2011 Remaster" for "Song". Two passes
    /// over the collection at most.
    pub fn find_tracks(&self, wanted: &[(&str, &str)]) -> Vec<Option<String>> {
        let mut found = vec![None; wanted.len()];
        let Ok(loaded) = self.loaded() else { return found };
        let index = &loaded.index;
        let artists: Vec<String> = wanted.iter().map(|(_, artist)| normalize(artist)).collect();
        for loose in [false, true] {
            let fold = |title: &str| if loose { normalize(&strip_noise(title)) } else { normalize(title) };
            let titles: Vec<String> = wanted.iter().map(|(title, _)| fold(title)).collect();
            let mut left = titles.iter().zip(&found).filter(|(title, found)| found.is_none() && !title.is_empty()).count();
            for row in 0..index.len() {
                if left == 0 {
                    break;
                }
                let title = fold(index.title.get(row));
                let mut theirs: Option<String> = None;
                for (at, (wanted_title, artist)) in titles.iter().zip(&artists).enumerate() {
                    if found[at].is_some() || wanted_title.is_empty() || *wanted_title != title {
                        continue;
                    }
                    let theirs = theirs.get_or_insert_with(|| normalize(index.artist_name(row as u32)));
                    if artist.is_empty() || theirs == artist || theirs.contains(artist.as_str()) || artist.contains(theirs.as_str()) {
                        found[at] = Some(index.ids[row].to_string());
                        left -= 1;
                    }
                }
            }
        }
        found
    }
}

fn take_in_one(writer: &mut Writer, arrival: &Arrival, playlists: &mut HashMap<String, String>) -> AppResult<String> {
    let path = arrival.path.as_path();
    if !path.is_file() {
        return Err(AppError::Validation(format!("{} is no longer there", path.display())));
    }
    let at = |writer: &Writer, path: &Path| writer.track_id_at(path).map_err(rekordbox_err);

    let previous = match arrival.replaces.as_deref().filter(|old| *old != path) {
        Some(old) => at(writer, old)?,
        None => None,
    };
    let id = if let Some(id) = previous {
        // The track follows its file: cues, grid and playlists stay with it.
        writer
            .relocate_with_undo(&[(id.clone(), path.to_path_buf())])
            .map_err(rekordbox_err)?;
        if !arrival.moved_only {
            writer.reload_tags(&id).map_err(rekordbox_err)?;
        }
        id
    } else if let Some(id) = at(writer, path)? {
        writer.reload_tags(&id).map_err(rekordbox_err)?;
        id
    } else {
        writer.import_file(path).map_err(rekordbox_err)?
    };

    if let Some(name) = arrival.playlist.as_deref() {
        let playlist = named_playlist(writer, name, playlists)?;
        writer.add_tracks(&playlist, std::slice::from_ref(&id)).map_err(rekordbox_err)?;
    }
    if let Some(playlist) = arrival.playlist_id.as_deref().filter(|p| is_plain_playlist(writer, p)) {
        writer.add_tracks(playlist, std::slice::from_ref(&id)).map_err(rekordbox_err)?;
    }
    Ok(id)
}

/// Whether `id` is a playlist that can be given tracks: not a folder, an
/// intelligent playlist, or gone.
fn is_plain_playlist(writer: &Writer, id: &str) -> bool {
    writer
        .library()
        .connection()
        .query_row("SELECT COUNT(*) FROM djmdPlaylist WHERE ID = ?1 AND Attribute = 0", [id], |r| r.get::<_, i64>(0))
        .is_ok_and(|n| n > 0)
}

/// The top-level playlist called `name`, made if there isn't one. `known`
/// remembers the answer for the rest of a batch.
fn named_playlist(writer: &mut Writer, name: &str, known: &mut HashMap<String, String>) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("The playlist needs a name".into()));
    }
    if let Some(id) = known.get(name) {
        return Ok(id.clone());
    }
    let id = match writer.child_named(ROOT, name).map_err(rekordbox_err)? {
        Some(id) => {
            // 0 is a plain playlist; folders and intelligent playlists can't
            // be given tracks.
            let attribute: i64 = writer
                .library()
                .connection()
                .query_row("SELECT Attribute FROM djmdPlaylist WHERE ID = ?1", [id.as_str()], |r| r.get(0))
                .map_err(rekordbox_err)?;
            if attribute != 0 {
                return Err(AppError::Rekordbox(format!(
                    "\"{name}\" in rekordbox is a folder or an intelligent playlist, so tracks can't be added to it"
                )));
            }
            id
        }
        None => writer.create_playlist(name, ROOT).map_err(rekordbox_err)?,
    };
    known.insert(name.to_owned(), id.clone());
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, query};
    use super::super::{PlaylistKind, Sort, Source};
    use super::*;

    /// A second of silence as a WAV file: enough for the tag reader.
    fn wav(path: &Path) {
        let (rate, samples) = (8000u32, 8000u32);
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + samples * 2).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // mono
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((samples * 2).to_le_bytes());
        bytes.resize(bytes.len() + samples as usize * 2, 0);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn members(collection: &Collection, playlist: &str) -> Vec<String> {
        let q = query(Source::Playlist { id: playlist.to_owned() }, Sort::TrackNo);
        collection.tracks(&q, 0, 1000).unwrap().rows.into_iter().map(|r| r.id).collect()
    }

    #[test]
    fn folders_and_files_are_imported_once() {
        let (dir, collection) = fixture();
        let music = dir.path().join("music");
        wav(&music.join("a/one.wav"));
        wav(&music.join("a/two.wav"));
        wav(&music.join("three.wav"));
        std::fs::write(music.join("cover.jpg"), "art").unwrap();
        let before = collection.status().unwrap().tracks;

        let report = collection.import(&[music.clone()], None).unwrap();
        assert_eq!((report.added, report.existing, report.skipped.len()), (3, 0, 0));
        assert_eq!(collection.status().unwrap().tracks, before + 3);
        // An untagged file is titled by its name.
        let id = collection.track_at(&music.join("three.wav")).expect("imported");
        assert_eq!(collection.path_of(&id), Some(music.join("three.wav")));

        let again = collection.import(&[music.join("three.wav"), music.join("a")], None).unwrap();
        assert_eq!((again.added, again.existing), (0, 3));
        assert_eq!(collection.status().unwrap().tracks, before + 3);

        assert!(collection.import(&[music.join("cover.jpg")], None).is_err());
    }

    #[test]
    fn arrivals_join_a_playlist_made_for_them() {
        let (dir, collection) = fixture();
        let music = dir.path().join("music");
        let (one, two) = (music.join("one.wav"), music.join("two.wav"));
        wav(&one);
        wav(&two);
        let arrival = |path: &Path| Arrival { path: path.to_path_buf(), playlist: Some("From Spotify".into()), ..Default::default() };

        let taken = collection.take_in(&[arrival(&one), arrival(&music.join("missing.wav"))]).unwrap();
        assert!(taken[0].is_ok() && taken[1].is_err());
        collection.take_in(&[arrival(&two), arrival(&one)]).unwrap();

        let nodes = collection.playlists().unwrap();
        let made: Vec<_> = nodes.iter().filter(|n| n.name == "From Spotify").collect();
        assert_eq!(made.len(), 1, "one playlist, reused");
        assert_eq!((made[0].kind, made[0].count), (PlaylistKind::Playlist, Some(2)));
        let ids = [collection.track_at(&one).unwrap(), collection.track_at(&two).unwrap()];
        assert_eq!(members(&collection, &made[0].id), ids);
    }

    #[test]
    fn arrivals_join_a_playlist_by_id_while_it_exists() {
        let (dir, collection) = fixture();
        let music = dir.path().join("music");
        let (one, two) = (music.join("one.wav"), music.join("two.wav"));
        wav(&one);
        wav(&two);
        let list = collection.add_to_named_playlist("Target", &[]).unwrap();

        let arrival = Arrival { path: one, playlist_id: Some(list.clone()), ..Default::default() };
        let id = collection.take_in(&[arrival]).unwrap().remove(0).expect("imported");
        assert_eq!(members(&collection, &list), vec![id]);

        // A playlist that has gone doesn't hold the file up.
        let arrival = Arrival { path: two, playlist_id: Some("987654321".into()), ..Default::default() };
        assert!(collection.take_in(&[arrival]).unwrap()[0].is_ok());
    }

    #[test]
    fn a_track_follows_the_file_that_replaces_it() {
        let (dir, collection) = fixture();
        let music = dir.path().join("music");
        let (old, new) = (music.join("old.wav"), music.join("sorted/new.wav"));
        wav(&old);
        let id = collection.import(&[old.clone()], None).unwrap().track_ids.remove(0);
        let list = collection.add_to_named_playlist("Keep", std::slice::from_ref(&id)).unwrap();
        let tracks = collection.status().unwrap().tracks;

        std::fs::remove_file(&old).unwrap();
        wav(&new);
        let taken = collection
            .take_in(&[Arrival { path: new.clone(), replaces: Some(old.clone()), ..Default::default() }])
            .unwrap();
        assert_eq!(taken, vec![Ok(id.clone())]);
        assert_eq!(collection.status().unwrap().tracks, tracks, "no second track");
        assert_eq!(collection.path_of(&id), Some(new));
        assert_eq!(collection.track_at(&old), None);
        assert_eq!(members(&collection, &list), vec![id]);
    }

    #[test]
    fn a_file_that_only_moved_keeps_what_rekordbox_has_for_it() {
        let (dir, collection) = fixture();
        let music = dir.path().join("music");
        let (old, new) = (music.join("elsewhere/old.wav"), music.join("library/new.wav"));
        wav(&old);
        let id = collection.import(std::slice::from_ref(&old), None).unwrap().track_ids.remove(0);
        let edit = super::super::Edit::SetField { track_id: id.clone(), field: "title".into(), value: "Named in rekordbox".into() };
        collection.edit(edit).unwrap();
        let title = |collection: &Collection| {
            let rows = collection.tracks(&query(Source::Collection, Sort::Title), 0, 1000).unwrap().rows;
            rows.into_iter().find(|row| row.id == id).unwrap().title
        };

        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::rename(&old, &new).unwrap();
        let moved = Arrival { path: new.clone(), replaces: Some(old), moved_only: true, ..Default::default() };
        assert_eq!(collection.take_in(&[moved]).unwrap(), vec![Ok(id.clone())]);
        assert_eq!(collection.path_of(&id), Some(new));
        // The file's tags have no title; reading them again would have put
        // its file's name there instead.
        assert_eq!(title(&collection), "Named in rekordbox");
    }

    /// Imports into a copy of the installed rekordbox library; the original
    /// is only copied, never opened. Run with
    /// `cargo test live_ingest -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_ingest() {
        let installed = rbl_db::detect().expect("rekordbox library");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("master.db");
        std::fs::copy(&installed.master_db, &copy).unwrap();
        let collection = Collection::new(dir.path().join("backups"));
        let location = rbl_db::LibraryLocation { master_db: copy, is_real_install: false, ..installed };
        let before = collection.load_from(location).unwrap();

        let (old, new) = (dir.path().join("music/old.wav"), dir.path().join("music/new.wav"));
        wav(&old);
        let arrival = Arrival { path: old.clone(), playlist: Some("jim-box test".into()), ..Default::default() };
        let id = collection.take_in(&[arrival]).unwrap().remove(0).expect("imported");
        let after = collection.status().unwrap();
        assert_eq!((after.tracks, after.playlists), (before.tracks + 1, before.playlists + 1));

        std::fs::rename(&old, &new).unwrap();
        let moved = Arrival { path: new.clone(), replaces: Some(old), ..Default::default() };
        assert_eq!(collection.take_in(&[moved]).unwrap(), vec![Ok(id.clone())]);
        assert_eq!(collection.path_of(&id), Some(new));
        assert_eq!(collection.status().unwrap().tracks, before.tracks + 1);
        println!("{} tracks before, track {id} imported, moved and listed", before.tracks);
    }

    #[test]
    fn named_playlists_are_not_confused_with_folders() {
        let (_dir, collection) = fixture();
        let q = query(Source::Collection, Sort::Title);
        let ids: Vec<String> = collection.tracks(&q, 0, 3).unwrap().rows.into_iter().map(|r| r.id).collect();
        collection
            .edit(super::super::Edit::CreatePlaylist { name: "Sets".into(), parent_id: None, folder: true })
            .unwrap();
        assert!(collection.add_to_named_playlist("Sets", &ids).is_err());
        assert!(collection.add_to_named_playlist("  ", &ids).is_err());

        let list = collection.add_to_named_playlist("Fresh", &ids[..2]).unwrap();
        assert_eq!(collection.add_to_named_playlist("Fresh", &ids).unwrap(), list);
        assert_eq!(members(&collection, &list), ids);
    }

    #[test]
    fn tracks_are_found_by_title_and_artist() {
        let (_dir, collection) = fixture();
        let row = collection.tracks(&query(Source::Collection, Sort::Title), 0, 1).unwrap().rows.remove(0);
        let upper = row.title.to_uppercase();
        let found = collection.find_tracks(&[
            (upper.as_str(), row.artist.as_str()),
            ("no such track anywhere", "nobody"),
            (row.title.as_str(), ""),
            ("", ""),
        ]);
        assert_eq!(found, vec![Some(row.id.clone()), None, Some(row.id), None]);
    }
}
