use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use super::{TrackMetadata, TrackRequest};

/// Tags already present in a file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub date: Option<String>,
    pub isrc: Option<String>,
    pub mb_recording_id: Option<String>,
    #[serde(default)]
    pub mb_release_id: Option<String>,
}

impl ExistingTags {
    /// Metadata as the tags currently describe the file, for comparing
    /// against where the path template would put it.
    pub fn as_metadata(&self) -> TrackMetadata {
        let artist = self.artist.clone().unwrap_or_default();
        TrackMetadata {
            title: self.title.clone().unwrap_or_default(),
            album: self.album.clone().unwrap_or_default(),
            album_artist: self.album_artist.clone().unwrap_or_else(|| artist.clone()),
            artist,
            track_number: self.track_number,
            disc_number: self.disc_number,
            date: self.date.clone(),
            isrc: self.isrc.clone(),
            mb_recording_id: self.mb_recording_id.clone(),
            mb_release_id: self.mb_release_id.clone(),
            ..Default::default()
        }
    }
}

/// The tags a file will have once `meta` is written to it.
impl From<&TrackMetadata> for ExistingTags {
    fn from(meta: &TrackMetadata) -> Self {
        Self {
            title: Some(meta.title.clone()),
            artist: Some(meta.artist.clone()),
            album: Some(meta.album.clone()),
            album_artist: Some(meta.album_artist.clone()),
            track_number: meta.track_number,
            disc_number: meta.disc_number,
            date: meta.date.clone(),
            isrc: meta.isrc.clone(),
            mb_recording_id: meta.mb_recording_id.clone(),
            mb_release_id: meta.mb_release_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// Matched by an ID (MusicBrainz or ISRC), or exact title, artist and length.
    High,
    /// Title and artist match but something else is uncertain.
    Medium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    /// Waiting to be identified.
    Pending,
    /// Nothing to fix: tagged as the match says and filed where it belongs.
    Organized,
    Matched,
    Unmatched,
    Applying,
    Applied,
    Failed,
}

/// The release a folder was matched to as a whole, shared by its files' proposals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumProposal {
    pub release_id: String,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    /// Files from the folder matched to the release's tracks.
    pub matched: usize,
    /// Tracks on the release.
    pub tracks: usize,
    /// How far the folder is from the release: 0 is a perfect match.
    pub distance: f64,
}

/// What the scan proposes doing to a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub metadata: TrackMetadata,
    pub confidence: Confidence,
    /// Where the file will live after applying (may equal its current path).
    pub target_path: PathBuf,
    /// The matching search result, used to fetch a lossless replacement.
    pub request: Option<TrackRequest>,
    /// Set when the file was matched as part of its album.
    #[serde(default)]
    pub album: Option<AlbumProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: Uuid,
    /// The collection track this file belongs to.
    pub track_id: String,
    pub path: PathBuf,
    /// Path relative to the music folder, for display.
    pub relative_path: String,
    pub format: String,
    pub lossless: bool,
    pub duration_secs: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bitrate: Option<u32>,
    /// File size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Last modified, in ms since the epoch.
    #[serde(default)]
    pub modified: Option<u64>,
    pub tags: ExistingTags,
    pub proposal: Option<Proposal>,
    pub status: ItemStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryPhase {
    Idle,
    Scanning,
    Matching,
    Applying,
    Moving,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryProgress {
    pub phase: LibraryPhase,
    pub done: usize,
    pub total: usize,
}

impl LibraryProgress {
    pub const IDLE: Self = Self { phase: LibraryPhase::Idle, done: 0, total: 0 };
}
