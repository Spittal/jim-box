//! What we know about a track before identifying it.

use std::path::Path;

use crate::state::{ExistingTags, TrackMetadata, TrackRequest};
use crate::text::strip_noise;

/// What we believe a track is before looking it up, from a search result, a
/// file's tags or a playlist entry. Steers matching and fills gaps in what
/// MusicBrainz returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackHint {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: Option<u32>,
    pub isrc: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub release_date: Option<String>,
    pub cover_xl_url: Option<String>,
}

impl TrackHint {
    /// A search query: artist and title, without annotations.
    pub fn query(&self) -> String {
        format!("{} {}", self.artist, strip_noise(&self.title)).trim().to_string()
    }
}

impl From<&TrackRequest> for TrackHint {
    fn from(request: &TrackRequest) -> Self {
        Self {
            title: request.title.clone(),
            artist: request.artist.clone(),
            album: request.album.clone(),
            duration_secs: Some(request.duration_secs),
            isrc: request.isrc.clone(),
            track_number: request.track_number,
            disc_number: request.disc_number,
            release_date: request.release_date.clone(),
            cover_xl_url: request.cover_xl_url.clone(),
        }
    }
}

impl From<&TrackMetadata> for TrackHint {
    fn from(meta: &TrackMetadata) -> Self {
        Self {
            title: meta.title.clone(),
            artist: meta.artist.clone(),
            album: meta.album.clone(),
            duration_secs: meta.duration_ms.map(|ms| (ms + 500) / 1000),
            isrc: meta.isrc.clone(),
            ..Default::default()
        }
    }
}

/// Builds a hint from tags, falling back to the `Artist/Album/NN Title.ext`
/// layout (or `Artist - Title.ext`) when tags are missing.
pub fn hint_from_file(tags: &ExistingTags, relative_path: &Path, duration_secs: Option<u32>) -> Option<TrackHint> {
    let stem = relative_path.file_stem()?.to_string_lossy();
    let dirs: Vec<String> = relative_path
        .parent()
        .map(|p| p.iter().map(|c| c.to_string_lossy().into_owned()).collect())
        .unwrap_or_default();

    let name = strip_track_number(&stem);
    let (name_artist, name_title) = match name.split_once(" - ") {
        Some((artist, title)) => (Some(artist.trim().to_string()), title.trim().to_string()),
        None => (None, name.trim().to_string()),
    };

    let title = tags.title.clone().unwrap_or(name_title);
    let artist = tags
        .artist
        .clone()
        .or_else(|| tags.album_artist.clone())
        .or(name_artist)
        .or_else(|| (dirs.len() >= 2).then(|| dirs[dirs.len() - 2].clone()))
        .unwrap_or_default();
    let album = tags.album.clone().or_else(|| dirs.last().cloned()).unwrap_or_default();

    (!title.is_empty()).then(|| TrackHint {
        title,
        artist,
        album,
        duration_secs,
        isrc: tags.isrc.clone(),
        track_number: tags.track_number,
        disc_number: tags.disc_number,
        release_date: tags.date.clone(),
        cover_xl_url: None,
    })
}

/// Drops a leading track number like "06 ", "06 - ", "06. " or "1-06 ".
fn strip_track_number(name: &str) -> &str {
    let digits = name.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    if digits.len() == name.len() || digits.is_empty() {
        return name;
    }
    let rest = digits.trim_start_matches(['.', ' ', '-', '_']);
    if rest.len() < digits.len() { rest } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_prefers_tags() {
        let tags = ExistingTags {
            title: Some("Alright".into()),
            artist: Some("Kendrick Lamar".into()),
            ..Default::default()
        };
        let hint = hint_from_file(&tags, Path::new("x/y/01 whatever.mp3"), Some(219)).unwrap();
        assert_eq!(hint.title, "Alright");
        assert_eq!(hint.artist, "Kendrick Lamar");
        assert_eq!(hint.album, "y");
    }

    #[test]
    fn hint_falls_back_to_folder_layout() {
        let hint = hint_from_file(
            &ExistingTags::default(),
            Path::new("Radiohead/In Rainbows/04 Weird Fishes.flac"),
            None,
        )
        .unwrap();
        assert_eq!(
            (hint.artist.as_str(), hint.album.as_str(), hint.title.as_str()),
            ("Radiohead", "In Rainbows", "Weird Fishes")
        );
    }

    #[test]
    fn hint_parses_artist_dash_title() {
        let hint = hint_from_file(&ExistingTags::default(), Path::new("Daft Punk - One More Time.mp3"), None).unwrap();
        assert_eq!((hint.artist.as_str(), hint.title.as_str()), ("Daft Punk", "One More Time"));
    }

    #[test]
    fn strips_track_numbers() {
        assert_eq!(strip_track_number("06 Title"), "Title");
        assert_eq!(strip_track_number("06 - Title"), "Title");
        assert_eq!(strip_track_number("1-06. Title"), "Title");
        assert_eq!(strip_track_number("1999"), "1999");
        assert_eq!(strip_track_number("22 Acacia Avenue"), "Acacia Avenue");
    }
}
