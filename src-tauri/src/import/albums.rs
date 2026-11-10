//! Grouping files into albums, as beets' importer does: a folder of
//! files sharing an album tag is one album, and disc subfolders ("CD1",
//! "Disc 2") join it. Albums are then matched as a whole by
//! [`autotag::match_album`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::autotag::{self, AlbumCandidate, FileTrack};
use crate::error::AppResult;
use crate::sources::Sources;
use crate::state::LibraryItem;
use crate::text::normalize;

/// Splits files into albums (folders of two or more files sharing an album
/// tag) and single files. Disc subfolders ("CD1", "Disc 2") join their album.
pub fn group(items: Vec<LibraryItem>) -> (Vec<Vec<LibraryItem>>, Vec<LibraryItem>) {
    let mut groups: HashMap<(PathBuf, String), Vec<LibraryItem>> = HashMap::new();
    let mut order = Vec::new();
    for item in items {
        let key = (album_dir(&item.path), normalize(item.tags.album.as_deref().unwrap_or_default()));
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(item);
    }

    let (mut albums, mut singles) = (Vec::new(), Vec::new());
    for key in order {
        let group = groups.remove(&key).unwrap_or_default();
        if group.len() >= 2 {
            albums.push(group);
        } else {
            singles.extend(group);
        }
    }
    (albums, singles)
}

fn album_dir(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    match parent.file_name() {
        Some(name) if is_disc_folder(&name.to_string_lossy()) => parent.parent().unwrap_or(parent).to_path_buf(),
        _ => parent.to_path_buf(),
    }
}

/// "CD1", "Disc 2", "disk_03"…
pub(super) fn is_disc_folder(name: &str) -> bool {
    let name = normalize(name).replace(' ', "");
    ["cd", "disc", "disk"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Matches a group of files to a release. `None` when no release fits well.
pub async fn identify(app: &AppHandle, items: &[LibraryItem], root: &Path) -> AppResult<Option<AlbumCandidate>> {
    let files: Vec<FileTrack> = items
        .iter()
        .map(|item| FileTrack::from_file(&item.tags, item.path.strip_prefix(root).unwrap_or(&item.path), item.duration_secs))
        .collect();
    let dir = album_dir(&items[0].path);
    let folder_name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    autotag::match_album(&app.state::<Sources>(), &files, folder_name.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ExistingTags, ItemStatus};
    use uuid::Uuid;

    fn item(path: &str, album: Option<&str>) -> LibraryItem {
        LibraryItem {
            id: Uuid::new_v4(),
            track_id: String::new(),
            path: PathBuf::from(path),
            relative_path: path.into(),
            format: "FLAC".into(),
            lossless: true,
            duration_secs: None,
            bit_depth: None,
            sample_rate: None,
            bitrate: None,
            size: 0,
            modified: None,
            tags: ExistingTags { album: album.map(str::to_string), ..Default::default() },
            proposal: None,
            status: ItemStatus::Pending,
            error: None,
        }
    }

    #[test]
    fn groups_folders_and_disc_subfolders() {
        let (albums, singles) = group(vec![
            item("/m/A/Album/CD1/01 a.flac", Some("Album")),
            item("/m/A/Album/CD2/01 b.flac", Some("Album")),
            item("/m/A/Other/01 c.flac", Some("Other")),
            item("/m/A/Other/02 d.flac", Some("Other")),
            item("/m/Singles/x.flac", Some("X")),
            item("/m/Singles/y.flac", Some("Y")),
        ]);
        let sizes: Vec<usize> = albums.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2]);
        assert_eq!(singles.len(), 2);
    }

    #[test]
    fn recognizes_disc_folders() {
        assert!(is_disc_folder("CD1"));
        assert!(is_disc_folder("Disc 2"));
        assert!(!is_disc_folder("Discovery"));
        assert!(!is_disc_folder("CD"));
    }
}
