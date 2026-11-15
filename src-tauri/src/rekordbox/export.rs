//! What a USB export takes from the library: the tracks of the playlists
//! chosen, each once, and the playlists and folders that hold them, as
//! rbxport's `rbl-export` wants them. Put together the way rbxport's own
//! app does for its Sync Manager.
//!
//! Nothing here writes: the library is opened read-only for what the index
//! doesn't keep (cues, My Tags, the library's id) and closed again.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rbl_db::export_info::{self, TrackExtras};
use rbl_db::OpenMode;
use rbl_export::{SourceMyTag, SourcePlaylist, SourceTrack, SyncNode, SyncSource};
use rbl_index::{Playlists, TrackSource, NO_ID};

use super::{rekordbox_err, Collection};
use crate::error::{AppError, AppResult};

/// What an export is asked to write.
pub struct Selection {
    pub tracks: Vec<SourceTrack>,
    /// The playlists, with the folders above them, in the tree's order.
    pub playlists: Vec<SourcePlaylist>,
    /// Every My Tag of the library, listed on the stick whole as rekordbox
    /// lists them.
    pub my_tags: Vec<SourceMyTag>,
    /// What the stick's sync record names: the library and its tree.
    pub sync: SyncSource,
}

impl Collection {
    /// The library's own id, which a stick written from it records.
    pub fn library_db_id(&self) -> AppResult<u64> {
        let loaded = self.loaded()?;
        let db = rbl_db::Library::open(loaded.location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        export_info::db_id(db.connection()).map_err(rekordbox_err)
    }

    /// The tracks and playlists an export of `playlist_ids` writes. A folder
    /// stands for every playlist under it, and goes on the stick as a folder
    /// with them inside; an intelligent playlist is what its rule admits
    /// now. Blocking.
    pub fn export_selection(&self, playlist_ids: &[String]) -> AppResult<Selection> {
        // Not while a change is being made: the index and the database would disagree.
        let _editing = self.editing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.refresh()?;
        let loaded = self.loaded()?;
        let library = &loaded.index;
        let share = loaded.location.share_root.as_path();

        // Named first and read after: `source_rows` takes the playlists itself.
        let mut named: Vec<(u64, String, TrackSource)> = Vec::new();
        // Folders asked for by name, kept even when nothing under them is a playlist.
        let mut chosen_folders: Vec<u64> = Vec::new();
        {
            let playlists = library.playlists();
            let mut seen = HashSet::new();
            for id in playlist_ids {
                let Some(index) = id.parse::<u64>().ok().and_then(|id| playlists.index_of(id)) else {
                    return Err(AppError::Rekordbox("That playlist is no longer in the library".into()));
                };
                let (leaves, folders) = if playlists.is_folder(index) {
                    folder_contents(&playlists, index)
                } else {
                    (vec![index], Vec::new())
                };
                chosen_folders.extend(folders.into_iter().filter_map(|folder| playlists.ids.get(folder).copied()));
                for index in leaves {
                    if !seen.insert(index) {
                        continue;
                    }
                    let source = if playlists.is_smart(index) {
                        TrackSource::SmartPlaylist(index)
                    } else {
                        TrackSource::Playlist(index)
                    };
                    named.push((playlists.ids.get(index).copied().unwrap_or(0), playlists.name(index).to_owned(), source));
                }
            }
        }
        let rows_of: Vec<Vec<u32>> = named.iter().map(|(_, _, source)| library.source_rows(source)).collect();

        // What the index doesn't hold, in one read for the whole selection.
        let ids: Vec<String> = rows_of
            .iter()
            .flatten()
            .map(|&row| library.ids.get(row as usize).copied().unwrap_or(0).to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let plain: Vec<u64> = named
            .iter()
            .filter(|(_, _, source)| matches!(source, TrackSource::Playlist(_)))
            .map(|(id, _, _)| *id)
            .collect();
        let db = rbl_db::Library::open(loaded.location.clone(), OpenMode::ReadOnly).map_err(rekordbox_err)?;
        let conn = db.connection();
        let my_tags = export_info::my_tags(conn).map_err(rekordbox_err)?;
        let extras = export_info::track_extras(conn, &ids).map_err(rekordbox_err)?;
        let db_id = export_info::db_id(conn).map_err(rekordbox_err)?;
        let cloud_db_id = export_info::cloud_db_id(conn).map_err(rekordbox_err)?;
        let entries_changed = export_info::playlist_entries_changed(conn, &plain).map_err(rekordbox_err)?;
        drop(db);

        let playlists = library.playlists();
        // The whole tree goes in the record, which keeps the chosen
        // playlists and the folders above them.
        let tree: Vec<SyncNode> = (0..playlists.len())
            .map(|index| SyncNode {
                id: playlists.ids.get(index).copied().unwrap_or(0),
                parent: match playlists.parent.get(index).copied() {
                    Some(parent) if parent != NO_ID => playlists.ids.get(parent as usize).copied().unwrap_or(0),
                    _ => 0,
                },
                attribute: playlists.attribute.get(index).copied().unwrap_or(0),
            })
            .collect();
        let parent_of: HashMap<u64, u64> = tree.iter().map(|node| (node.id, node.parent)).collect();

        let mut tracks: Vec<SourceTrack> = Vec::new();
        let mut position: HashMap<u32, usize> = HashMap::new();
        let mut lists = Vec::with_capacity(named.len());
        for ((id, name, _), rows) in named.into_iter().zip(rows_of) {
            let mut track_indices = Vec::with_capacity(rows.len());
            for row in rows {
                let at = *position.entry(row).or_insert_with(|| {
                    let content = library.ids.get(row as usize).copied().unwrap_or(0).to_string();
                    let extra = extras.get(&content).cloned().unwrap_or_default();
                    tracks.push(source_track(library, share, row, &extra));
                    tracks.len() - 1
                });
                track_indices.push(at);
            }
            lists.push(SourcePlaylist {
                id,
                name,
                parent_id: parent_of.get(&id).copied().unwrap_or(0),
                track_indices,
                entries_changed_ms: entries_changed.get(&id).copied(),
                ..Default::default()
            });
        }

        // The folders above each playlist are rows on the stick too.
        let mut ancestors = HashSet::new();
        let starts = lists.iter().map(|list| list.parent_id).chain(chosen_folders);
        for start in starts {
            let mut next = start;
            while next != 0 && ancestors.insert(next) {
                next = parent_of.get(&next).copied().unwrap_or(0);
            }
        }
        for id in ancestors {
            let index = playlists
                .index_of(id)
                .ok_or_else(|| AppError::Rekordbox("A playlist's folder is missing from the library".into()))?;
            lists.push(SourcePlaylist {
                id,
                name: playlists.name(index).to_owned(),
                parent_id: parent_of.get(&id).copied().unwrap_or(0),
                folder: true,
                ..Default::default()
            });
        }
        // The exporter numbers siblings in the order given: the tree's own.
        let rank = tree_rank(&playlists);
        lists.sort_by_key(|list| rank.get(&list.id).copied().unwrap_or(usize::MAX));

        Ok(Selection {
            tracks,
            playlists: lists,
            my_tags: my_tags.iter().map(source_my_tag).collect(),
            sync: SyncSource { db_id, tree, automatic: false, cloud_db_id },
        })
    }
}

/// What a folder stands for, in tree order: the playlists under it at any
/// depth, and the folder itself with every folder under it, so empty ones
/// keep their place.
fn folder_contents(playlists: &Playlists, folder: usize) -> (Vec<usize>, Vec<usize>) {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); playlists.len()];
    for index in 0..playlists.len() {
        if let Some(parent) = playlists.parent.get(index).copied().filter(|&p| p != NO_ID) {
            if let Some(bucket) = children.get_mut(parent as usize) {
                bucket.push(index);
            }
        }
    }
    let (mut leaves, mut folders) = (Vec::new(), Vec::new());
    // With a visited set: a corrupt parent cycle must not loop forever.
    let mut visited = vec![false; playlists.len()];
    let mut stack = vec![folder];
    while let Some(index) = stack.pop() {
        match visited.get_mut(index) {
            Some(seen) if !*seen => *seen = true,
            _ => continue,
        }
        if playlists.is_folder(index) {
            folders.push(index);
            stack.extend(children[index].iter().rev());
        } else {
            leaves.push(index);
        }
    }
    (leaves, folders)
}

/// Each playlist and folder id's place in the tree read top to bottom: a
/// folder, then what is inside it, each level in its stored order.
fn tree_rank(playlists: &Playlists) -> HashMap<u64, usize> {
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    for index in 0..playlists.len() {
        let parent = playlists.parent.get(index).copied().unwrap_or(NO_ID);
        children.entry(parent).or_default().push(index);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|&i| (playlists.seq.get(i).copied().unwrap_or(0), i));
    }
    let mut rank = HashMap::new();
    let mut visited = vec![false; playlists.len()];
    let mut stack: Vec<usize> = children.get(&NO_ID).map(|c| c.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(index) = stack.pop() {
        match visited.get_mut(index) {
            Some(seen) if !*seen => *seen = true,
            _ => continue,
        }
        rank.insert(playlists.ids.get(index).copied().unwrap_or(0), rank.len());
        if let Some(under) = u32::try_from(index).ok().and_then(|i| children.get(&i)) {
            stack.extend(under.iter().rev());
        }
    }
    rank
}

fn source_my_tag(tag: &export_info::MyTagRow) -> SourceMyTag {
    SourceMyTag {
        id: tag.id.parse().unwrap_or(0),
        seq: u32::try_from(tag.seq.max(0)).unwrap_or(u32::MAX),
        name: tag.name.clone(),
        attribute: u8::try_from(tag.attribute.clamp(0, 255)).unwrap_or(0),
        parent: tag.parent.parse().unwrap_or(0),
    }
}

/// Where a track's audio is: where the library says, or failing that one of
/// the other places a cloud-synced file may be.
fn source_audio(folder_path: &str, alternates: &[String]) -> PathBuf {
    let first = PathBuf::from(folder_path);
    if first.is_file() {
        return first;
    }
    alternates.iter().map(PathBuf::from).find(|p| p.is_file()).unwrap_or(first)
}

/// A path in the share folder, from the library's record of it.
fn in_share(share: &Path, relative: &str) -> Option<PathBuf> {
    (!relative.is_empty()).then(|| share.join(relative.trim_start_matches(['/', '\\'])))
}

/// One library row as the export wants it. The analysis is read from the
/// share folder when the track is written, not here.
fn source_track(library: &rbl_index::Library, share: &Path, row: u32, extra: &TrackExtras) -> SourceTrack {
    let i = row as usize;
    // What analysis wrote, from the row as it is now rather than as indexed.
    let analysed = extra.analysis.as_ref();
    let analysis_path = analysed.map_or_else(|| library.analysis_path.get(i), |a| a.analysis_path.as_str());
    let artwork_path = analysed.map_or_else(|| library.artwork_path.get(i), |a| a.artwork_path.as_str());
    SourceTrack {
        cues: Some(extra.cues.clone()),
        metadata: extra.metadata.clone(),
        // How a second export to the same stick knows a track it has written.
        id: library.ids.get(i).copied().unwrap_or(0),
        source_path: source_audio(library.folder_path.get(i), &extra.alternate_paths),
        artwork: in_share(share, artwork_path),
        my_tags: extra.my_tags.iter().filter_map(|t| t.parse().ok()).collect(),
        title: library.title.get(i).to_owned(),
        artist: library.artist_name(row).to_owned(),
        album: library.album_name(row).to_owned(),
        genre: library.genre_name(row).to_owned(),
        label: library.label_name(row).to_owned(),
        key: analysed.map_or_else(|| library.key_name(row).to_owned(), |a| a.key.clone()),
        comment: library.comment.get(i).to_owned(),
        date_added: library.date_added.get(i).to_owned(),
        release_date: library.release_date.get(i).to_owned(),
        bpm_x100: analysed.map_or_else(|| library.bpm_x100.get(i).copied().unwrap_or(0), |a| a.bpm_x100),
        duration_sec: u16::try_from(
            analysed.map_or_else(|| library.length_sec.get(i).copied().unwrap_or(0), |a| a.length_sec),
        )
        .unwrap_or(u16::MAX),
        rating: library.rating.get(i).copied().unwrap_or(0),
        color_id: library.color.get(i).copied().unwrap_or(0),
        bitrate: library.bitrate.get(i).copied().unwrap_or(0),
        sample_rate: library.sample_rate.get(i).copied().unwrap_or(0),
        file_size: library.file_size.get(i).copied().unwrap_or(0),
        year: library.year.get(i).copied().unwrap_or(0),
        analysis_source: in_share(share, analysis_path),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;

    /// A playlist's tracks are listed once each, under the folders it's in.
    #[test]
    fn a_playlist_is_selected_with_its_folders_and_each_track_once() {
        let (_dir, collection) = fixture();
        let nodes = collection.playlists().unwrap();
        let list = nodes.iter().find(|n| n.count.is_some_and(|c| c > 0)).expect("a playlist with tracks");
        let selection = collection.export_selection(std::slice::from_ref(&list.id)).unwrap();

        let members: std::collections::HashSet<String> =
            collection.playlist_members(&list.id).unwrap().into_iter().collect();
        let selected: std::collections::HashSet<String> = selection.tracks.iter().map(|t| t.id.to_string()).collect();
        assert_eq!(selected, members);
        assert_eq!(selection.tracks.len(), members.len());
        let leaves: Vec<_> = selection.playlists.iter().filter(|p| !p.folder).collect();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].name, list.name);
        assert_eq!(selection.playlists.iter().filter(|p| p.folder).count(), list.depth);

        // Asked for twice, it's still there once.
        let again = collection.export_selection(&[list.id.clone(), list.id.clone()]).unwrap();
        assert_eq!(again.tracks.len(), selection.tracks.len());
        assert_eq!(again.playlists.len(), selection.playlists.len());

        assert!(collection.export_selection(&["0".into()]).is_err());
    }

    /// A second of silence as a WAV file, which stands for a track's audio.
    fn write_wav(path: &std::path::Path) {
        let data = vec![0u8; 44_100 * 4];
        let mut wav = Vec::with_capacity(44 + data.len());
        wav.extend(b"RIFF");
        wav.extend((36 + data.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(44_100u32.to_le_bytes());
        wav.extend((44_100u32 * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        std::fs::write(path, wav).unwrap();
    }

    /// What's selected goes on a stick that reads back right, and a second
    /// export of the same finds nothing to do.
    #[test]
    fn a_selection_exports_to_a_stick_that_verifies() {
        let (dir, collection) = fixture();
        let nodes = collection.playlists().unwrap();
        let list = nodes.iter().find(|n| n.count.is_some_and(|c| c > 0)).expect("a playlist with tracks");
        let mut selection = collection.export_selection(std::slice::from_ref(&list.id)).unwrap();
        // The fixture's tracks have no files: give them some.
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        for (i, track) in selection.tracks.iter_mut().enumerate() {
            track.source_path = music.join(format!("{i}.wav"));
            write_wav(&track.source_path);
        }

        let stick = tempfile::tempdir().unwrap();
        let export = || {
            rbl_export::export_cancellable(
                stick.path(),
                &selection.tracks,
                &selection.playlists,
                &selection.my_tags,
                &rbl_export::ExportOptions { sync: Some(&selection.sync), ..Default::default() },
                &mut |_| {},
                &|| false,
            )
            .unwrap()
        };
        let report = export();
        assert!(report.verified);
        assert_eq!(report.tracks, selection.tracks.len());
        assert_eq!(report.tracks_added, selection.tracks.len());
        assert!(report.skipped.is_empty() && report.failed.is_empty(), "{:?} {:?}", report.skipped, report.failed);
        assert!(stick.path().join("PIONEER/rekordbox/export.pdb").is_file());
        assert!(stick.path().join("PIONEER/rekordbox/exportLibrary.db").is_file());
        assert!(rbl_export::verify(stick.path()).unwrap().is_ok());

        // The stick remembers the library and the playlist it was written with.
        let manifest = rbl_export::Manifest::load(stick.path()).unwrap();
        assert_eq!(manifest.db_id, selection.sync.db_id);
        assert!(manifest.playlists.iter().any(|p| !p.folder && p.library_id.to_string() == list.id));

        let again = export();
        assert_eq!(again.tracks_added, 0);
        assert_eq!(again.bytes_copied, 0);
    }
}
