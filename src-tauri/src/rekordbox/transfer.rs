//! Bringing tracks into a library from another one: a USB stick, or
//! another `master.db`. Whatever they come from wins: a track the library
//! already has takes the incoming beat grid, waveforms, cues, rating,
//! colour and comment; one it doesn't is copied in first, then given them.
//!
//! The same is done by rekordbox's Import Playlist for the tracks it copies
//! off a stick (see rbxport's `device_library.rs`), here also for the ones
//! the library has, so a stick played and cued on CDJs brings that back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbl_db::write::{DeviceAnalysis, ImportOutcome, Writer, ATTRIBUTE_FOLDER, ROOT};
use rbl_db::OpenMode;
use rbl_export::device_import::{candidate, Copies};
use serde::Serialize;

use super::analyse::replace_file;
use super::{local_path, prepare, rekordbox_err, Collection};
use crate::error::AppResult;

/// A track on its way in, with everything about it that comes too.
#[derive(Debug, Clone, Default)]
pub struct Incoming {
    /// Its id in the library it comes from.
    pub source_id: String,
    /// Its audio, where it is now.
    pub file: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_sec: u32,
    pub bpm_x100: u32,
    /// The key by name; empty for none.
    pub key: String,
    pub rating: u8,
    /// rekordbox's colour, 1 to 8; 0 for none.
    pub color: u8,
    pub comment: String,
    /// Its `.DAT`, `.EXT` and `.2EX`, when it has been analysed.
    pub analysis: Option<[PathBuf; 3]>,
    pub cues: Vec<rbl_anlz::CueEntry>,
    /// The library it was first exported from (its `DBID`) and its id
    /// there, when that's known: the surest way to find it again.
    pub identity: Option<(u64, u64)>,
}

/// Where tracks brought in go, besides the collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Collection,
    /// Added to the end of a playlist.
    Playlist(String),
    /// A playlist found by name under these folders (from `under`, or the
    /// top level), made where there's none, and made to hold exactly what's
    /// brought in, in its order.
    Named { under: Option<String>, folders: Vec<String>, name: String },
}

/// Some of the tracks brought in, by their place among them, and where they go.
pub type Placement = (Destination, Vec<usize>);

/// Placements of tracks named by their ids in the library they come from,
/// as places among `items`. Ones that aren't among them are left out.
pub fn placements(items: &[Incoming], wanted: Vec<(Destination, Vec<String>)>) -> Vec<Placement> {
    let at: HashMap<&str, usize> = items.iter().enumerate().map(|(at, item)| (item.source_id.as_str(), at)).collect();
    wanted
        .into_iter()
        .map(|(into, ids)| (into, ids.iter().filter_map(|id| at.get(id.as_str()).copied()).collect()))
        .collect()
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    /// Tracks the library didn't have, copied in.
    pub added: usize,
    /// Tracks it had, brought up to date.
    pub updated: usize,
    /// "title: reason" for each that couldn't be brought in, or only partly.
    pub failed: Vec<String>,
    /// The library's ids for what came in, in the order given.
    pub track_ids: Vec<String>,
    /// The playlist they went into, when they went into one: the first's,
    /// when they went into several.
    pub playlist_id: Option<String>,
}

/// How far apart two lengths can be for two files to be taken as the same song.
const SAME_LENGTH_SECS: u32 = 3;

/// Where a copy of `file` goes under `root`: the artist's folder, then the
/// album's, named the way an export names them.
fn copy_place(root: &Path, artist: &str, album: &str, file: &Path) -> PathBuf {
    let name = file.file_name().map_or_else(|| "track".into(), |n| n.to_string_lossy().into_owned());
    let placed = candidate(Path::new(""), artist, album, &name);
    root.join(placed.strip_prefix("Contents").unwrap_or(&placed))
}

impl Collection {
    /// These tracks of this library as they'd go into another. Blocking.
    pub fn incoming(&self, track_ids: &[String]) -> AppResult<Vec<Incoming>> {
        let loaded = self.loaded_or_read()?;
        let index = &loaded.index;
        let db = rbl_db::Library::open(loaded.location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        let db_id = rbl_db::export_info::db_id(db.connection()).unwrap_or(0);
        let mut out = Vec::with_capacity(track_ids.len());
        for id in track_ids {
            let Some(row) = index.row_of(id) else { continue };
            let r = row as usize;
            let relative = index.analysis_path.get(r);
            let analysis = (!relative.is_empty()).then(|| {
                let dat = rbl_anlz::resolve(&loaded.location.share_root, relative);
                [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")]
            });
            out.push(Incoming {
                source_id: id.clone(),
                file: PathBuf::from(local_path(index.folder_path.get(r))),
                title: index.title.get(r).to_owned(),
                artist: index.artist_name(row).to_owned(),
                album: index.album_name(row).to_owned(),
                duration_sec: index.length_sec[r],
                bpm_x100: index.bpm_x100[r],
                key: index.key_name(row).to_owned(),
                rating: index.rating[r],
                color: index.color[r],
                comment: index.comment.get(r).to_owned(),
                analysis,
                cues: cue_entries(&db, id)?,
                identity: id.parse().ok().filter(|_| db_id != 0).map(|id| (db_id, id)),
            });
        }
        Ok(out)
    }

    /// Brings tracks in from another library, copying the files of the ones
    /// this library hasn't got into `copy_to`, and puts them where `into`
    /// says. Blocking, for as long as the copying takes.
    pub fn bring_in(&self, items: &[Incoming], into: &Destination, copy_to: &Path) -> AppResult<TransferReport> {
        self.bring_in_to(items, &[(into.clone(), (0..items.len()).collect())], copy_to)
    }

    /// [`Self::bring_in`], putting each part of what comes in in a place of its own.
    pub fn bring_in_to(&self, items: &[Incoming], placements: &[Placement], copy_to: &Path) -> AppResult<TransferReport> {
        let mut report = TransferReport::default();
        self.refresh()?;
        let loaded = self.loaded_or_read()?;
        let mine = self.library_db_id().unwrap_or(0);

        // Which of them the library has: by where they came from, by their
        // file, or failing those by title and artist at about the same length.
        let mut found: Vec<Option<(String, PathBuf)>> = items
            .iter()
            .map(|item| {
                let id = item
                    .identity
                    .filter(|(db, _)| mine != 0 && *db == mine)
                    .map(|(_, id)| id.to_string())
                    .filter(|id| loaded.index.row_of(id).is_some())
                    .or_else(|| loaded.by_path.get(item.file.to_string_lossy().as_ref()).cloned())?;
                let row = loaded.index.row_of(&id)? as usize;
                Some((id, PathBuf::from(local_path(loaded.index.folder_path.get(row)))))
            })
            .collect();
        let unmatched: Vec<usize> = (0..items.len()).filter(|&at| found[at].is_none()).collect();
        let names: Vec<(&str, &str)> = unmatched.iter().map(|&at| (items[at].title.as_str(), items[at].artist.as_str())).collect();
        for (&at, id) in unmatched.iter().zip(self.find_tracks(&names)) {
            let Some(row) = id.as_deref().and_then(|id| loaded.index.row_of(id)) else { continue };
            let row = row as usize;
            if loaded.index.length_sec[row].abs_diff(items[at].duration_sec) <= SAME_LENGTH_SECS {
                found[at] = Some((id.unwrap_or_default(), PathBuf::from(local_path(loaded.index.folder_path.get(row)))));
            }
        }
        let share_root = loaded.location.share_root.clone();
        drop(loaded);

        // The files of the rest, copied in, each checked against the original.
        let mut copies = Copies::new();
        let mut copied: Vec<(usize, PathBuf)> = Vec::new();
        for (at, item) in items.iter().enumerate().filter(|(at, _)| found[*at].is_none()) {
            if !item.file.is_file() {
                report.failed.push(format!("{}: its file is missing ({})", item.title, item.file.display()));
                continue;
            }
            let wanted = copy_place(copy_to, &item.artist, &item.album, &item.file);
            let placed = match Copies::existing_copy(&item.file, &wanted) {
                Ok(Some(earlier)) => Ok(earlier),
                _ => copies.copy(&item.file, &wanted),
            };
            match placed {
                Ok(at_path) => copied.push((at, at_path)),
                Err(e) => report.failed.push(format!("{}: it couldn't be copied ({e})", item.title)),
            }
        }

        let written = self.write(|writer, _| {
            let files: Vec<PathBuf> = copied.iter().map(|(_, path)| path.clone()).collect();
            let outcomes = if files.is_empty() { Vec::new() } else { writer.import_files(&files).map_err(rekordbox_err)? };
            for ((at, path), outcome) in copied.iter().zip(outcomes) {
                match outcome {
                    ImportOutcome::Added(id) => {
                        report.added += 1;
                        found[*at] = Some((id, path.clone()));
                    }
                    ImportOutcome::Existing(id) => found[*at] = Some((id, path.clone())),
                    ImportOutcome::Refused(reason) => report.failed.push(format!("{}: {reason}", items[*at].title)),
                }
            }
            for (item, target) in items.iter().zip(&found) {
                let Some((id, audio)) = target else { continue };
                if let Err(e) = carry(writer, &share_root, id, audio, item) {
                    report.failed.push(format!("{}: {e}", item.title));
                }
                if !report.track_ids.contains(id) {
                    report.track_ids.push(id.clone());
                }
            }
            for (into, members) in placements {
                let ids: Vec<String> = members.iter().filter_map(|&at| found.get(at)?.as_ref().map(|(id, _)| id.clone())).collect();
                let playlist = place(writer, &ids, into)?;
                report.playlist_id = report.playlist_id.take().or(playlist);
            }
            Ok(())
        });
        if let Err(e) = written {
            if let Err(left) = copies.roll_back() {
                tracing::warn!("Couldn't remove the copies of an import that failed: {left}");
            }
            return Err(e);
        }
        report.updated = report.track_ids.len().saturating_sub(report.added);
        self.grids.forget(&report.track_ids);
        self.forget_view();
        Ok(report)
    }
}

/// Gives a track of the library what came with `item`: its analysis files
/// (with their path made the track's), BPM, key, cues, rating, colour and
/// comment, replacing what it had.
fn carry(writer: &mut Writer, share_root: &Path, id: &str, audio: &Path, item: &Incoming) -> AppResult<()> {
    let mut relative = None;
    if let Some([dat, ext, two_ex]) = &item.analysis {
        if let Ok(read) = rbl_anlz::Anlz::read(dat) {
            let path = writer.analysis_data_path_for(id).map_err(rekordbox_err)?;
            let target = rbl_anlz::resolve(share_root, &path);
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let ppth = rbl_anlz::share_tree_ppth(&audio.to_string_lossy());
            replace_file(&target, &read.with_path(&ppth))?;
            for (extension, source) in [("EXT", ext), ("2EX", two_ex)] {
                let to = rbl_anlz::sibling(&target, extension);
                match rbl_anlz::Anlz::read(source) {
                    Ok(read) => replace_file(&to, &read.with_path(&ppth))?,
                    // An older one of the track's own would disagree with the new grid.
                    Err(_) => {
                        let _ = std::fs::remove_file(&to);
                    }
                }
            }
            relative = Some(path);
        }
    }
    let key = Some(item.key.trim()).filter(|k| !k.is_empty());
    let analysis = DeviceAnalysis { bpm_x100: item.bpm_x100, key, analysis_data_path: relative.as_deref(), cues: &[] };
    writer.import_device_analysis(id, &analysis).map_err(rekordbox_err)?;
    // The ones the library can hold; the rest (a hot cue past P) are left out.
    let cues: Vec<rbl_anlz::CueEntry> = item
        .cues
        .iter()
        .filter(|c| c.hot_cue <= 16 && matches!(c.kind, 1 | 2) && (c.kind != 2 || c.loop_time_ms > c.time_ms))
        .cloned()
        .collect();
    writer.import_usb_cues(id, &cues, item.bpm_x100).map_err(rekordbox_err)?;
    writer.set_rating(id, item.rating.min(5)).map_err(rekordbox_err)?;
    let color = item.color.to_string();
    writer.set_color(id, (1..=8).contains(&item.color).then_some(color.as_str())).map_err(rekordbox_err)?;
    // A blank one is most likely one that wasn't carried, not one taken away.
    if !item.comment.trim().is_empty() {
        writer.set_comment(id, &item.comment).map_err(rekordbox_err)?;
    }
    Ok(())
}

/// Puts tracks where `into` says. Returns the playlist they went into.
fn place(writer: &mut Writer, track_ids: &[String], into: &Destination) -> AppResult<Option<String>> {
    match into {
        Destination::Collection => Ok(None),
        Destination::Playlist(id) => {
            if !track_ids.is_empty() {
                writer.add_tracks(id, track_ids).map_err(rekordbox_err)?;
            }
            Ok(Some(id.clone()))
        }
        Destination::Named { under, folders, name } => {
            let mut parent = under.clone().unwrap_or_else(|| ROOT.to_owned());
            for folder in folders {
                parent = match child(writer, &parent, folder, true)? {
                    Some(id) => id,
                    None => writer.create_folder(folder, &parent).map_err(rekordbox_err)?,
                };
            }
            let playlist = match child(writer, &parent, name, false)? {
                Some(id) => id,
                None => writer.create_playlist(name, &parent).map_err(rekordbox_err)?,
            };
            writer.set_tracks(&playlist, track_ids).map_err(rekordbox_err)?;
            Ok(Some(playlist))
        }
    }
}

/// The folder (or plain playlist) under `parent` called `name`.
fn child(writer: &Writer, parent: &str, name: &str, folder: bool) -> AppResult<Option<String>> {
    let Some(id) = writer.child_named(parent, name).map_err(rekordbox_err)? else { return Ok(None) };
    let attribute: i64 = writer
        .library()
        .connection()
        .query_row("SELECT COALESCE(Attribute, 0) FROM djmdPlaylist WHERE ID = ?1", [&id], |r| r.get(0))
        .map_err(rekordbox_err)?;
    // A plain playlist is 0; an intelligent one, 4, isn't one to fill.
    let fits = if folder { attribute == i64::from(ATTRIBUTE_FOLDER) } else { attribute == 0 };
    Ok(fits.then_some(id))
}

/// A track's cues as an analysis file lists them, from the library's rows.
fn cue_entries(db: &rbl_db::Library, track_id: &str) -> AppResult<Vec<rbl_anlz::CueEntry>> {
    let mut statement = db
        .connection()
        .prepare(
            "SELECT COALESCE(Kind, 0), COALESCE(InMsec, 0), COALESCE(OutMsec, -1), COALESCE(Comment, ''),
                    COALESCE(ColorTableIndex, 0), COALESCE(Color, -1)
             FROM djmdCue WHERE ContentID = ?1 AND rb_local_deleted = 0 ORDER BY InMsec",
        )
        .map_err(rekordbox_err)?;
    let rows = statement
        .query_map([track_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?))
        })
        .map_err(rekordbox_err)?;
    let mut cues = Vec::new();
    for row in rows {
        let (kind, start, end, comment, table, color) = row.map_err(rekordbox_err)?;
        let time_ms = u32::try_from(start).unwrap_or(0);
        let loop_end = u32::try_from(end).ok().filter(|&end| end > time_ms);
        let slot = prepare::slot_of(kind);
        let hot_cue = slot.map_or(0, |slot| u32::from(slot) + 1);
        cues.push(rbl_anlz::CueEntry {
            hot_cue,
            active_loop: kind == 4,
            kind: if loop_end.is_some() { 2 } else { 1 },
            time_ms,
            loop_time_ms: loop_end.unwrap_or(0),
            color_id: if hot_cue == 0 { rbl_anlz::cues::memory_colour_row(color) } else { u8::try_from(table).unwrap_or(0) },
            comment: Some(comment).filter(|c| !c.is_empty()),
            color_code: (hot_cue > 0).then(|| u8::try_from(table).unwrap_or(0)),
            rgb: None,
        });
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekordbox::tests::fixture;

    fn wav(path: &Path, secs: u32) {
        crate::rekordbox::analyse::tests::click_track(path, 120.0, secs);
    }

    #[test]
    fn copies_land_under_the_artist_and_album() {
        let placed = copy_place(Path::new("/music/Imported"), "Daft Punk", "Discovery", Path::new("/stick/Contents/x/One More Time.mp3"));
        assert!(placed.starts_with("/music/Imported"));
        assert!(placed.ends_with("One More Time.mp3"));
        assert!(placed.to_string_lossy().contains("Daft Punk"));
    }

    #[test]
    fn tracks_come_in_from_another_library_with_their_cues_and_win_over_the_ones_here() {
        let (_dir, from) = fixture();
        let (dir, to) = fixture();
        let source = tempfile::tempdir().unwrap();
        // A real file for one of the other library's tracks, cued.
        let file = source.path().join("song.wav");
        wav(&file, 20);
        let imported = from.import(&[file.clone()], None).unwrap();
        let id = imported.track_ids[0].clone();
        from.edit(crate::rekordbox::Edit::AddCue { track_id: id.clone(), slot: Some(0), position_ms: 1000, end_ms: None }).unwrap();
        from.edit(crate::rekordbox::Edit::SetRating { track_id: id.clone(), stars: 4 }).unwrap();

        let incoming = from.incoming(&[id]).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].cues.len(), 1);
        assert_eq!(incoming[0].cues[0].hot_cue, 1);

        let copy_to = dir.path().join("Imported");
        let into = Destination::Named { under: None, folders: vec!["From USB".into()], name: "Friday".into() };
        let report = to.bring_in(&incoming, &into, &copy_to).unwrap();
        assert_eq!((report.added, report.updated), (1, 0), "{:?}", report.failed);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let new = report.track_ids[0].clone();
        let row = to.rows_of(&[new.clone()]).unwrap().remove(0);
        assert_eq!(row.rating, 4);
        assert!(Path::new(&row.path).starts_with(&copy_to));
        assert_eq!(to.preparation(&new).unwrap().cues.len(), 1);
        let playlist = report.playlist_id.clone().unwrap();
        assert_eq!(to.playlist_members(&playlist).unwrap(), [new.clone()]);
        let names: Vec<String> = to.playlists().unwrap().into_iter().map(|p| p.name).collect();
        assert!(names.contains(&"From USB".to_owned()) && names.contains(&"Friday".to_owned()));

        // Again, with the other library's copy changed: the same track, brought up to date.
        let mut changed = incoming.clone();
        changed[0].rating = 2;
        changed[0].cues.clear();
        let again = to.bring_in(&changed, &into, &copy_to).unwrap();
        assert_eq!((again.added, again.updated), (0, 1), "{:?}", again.failed);
        assert_eq!(again.track_ids, [new.clone()]);
        assert_eq!(to.rows_of(&[new.clone()]).unwrap()[0].rating, 2);
        assert!(to.preparation(&new).unwrap().cues.is_empty());
        assert_eq!(again.playlist_id, Some(playlist));
    }
}
