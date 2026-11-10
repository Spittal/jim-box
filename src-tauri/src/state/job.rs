use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use super::{now_ms, AudioFormat, TrackMetadata, TrackRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Identifying,
    Searching,
    /// No peer answered any search; searching again at `retry_at`.
    Waiting,
    Downloading,
    Tagging,
    Completed,
    /// No acceptable lossless file was found on Soulseek.
    NotFound,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::NotFound | Self::Failed | Self::Cancelled
        )
    }
}

/// A Soulseek file that could satisfy a job, with its ranking score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub username: String,
    pub filename: String,
    pub size: u64,
    pub format: AudioFormat,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    /// Kilobits per second, as the peer reports it.
    #[serde(default)]
    pub bit_rate: Option<u32>,
    pub length_secs: Option<u32>,
    pub has_free_upload_slot: bool,
    pub queue_length: u64,
    pub upload_speed: u64,
    pub score: i64,
    /// Set once a download attempt from this candidate has failed.
    #[serde(default)]
    pub failed: bool,
    /// Why the file failed a check, for files offered only for manual picking.
    #[serde(default)]
    pub rejected: Option<String>,
}

/// Where a job looks for the track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobSource {
    #[default]
    Soulseek,
    /// YouTube Music, picked by hand when Soulseek has no copy. Lossy.
    Youtube,
}

/// The YouTube Music upload a job downloaded, or is downloading.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeSource {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub url: String,
    pub length_secs: Option<u32>,
    /// kbps once downloaded: about 256 with Premium cookies, else 128.
    pub bit_rate: Option<u32>,
}

/// One Soulseek search a job ran, so a "not found" can be told apart from
/// "nobody answered".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchAttempt {
    pub query: String,
    /// Peers that responded.
    pub peers: usize,
    /// Files that passed format, title, version and length checks.
    pub usable: usize,
    /// Lossless files with the right title that failed a check.
    #[serde(default)]
    pub rejected: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub bytes_transferred: u64,
    pub size: u64,
    /// Bytes per second.
    pub speed: f64,
    pub place_in_queue: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: Uuid,
    pub request: TrackRequest,
    pub metadata: Option<TrackMetadata>,
    pub status: JobStatus,
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub searches: Vec<SearchAttempt>,
    pub active_candidate: Option<usize>,
    /// slskd's id for the in-flight transfer, so it can be cancelled.
    pub transfer_id: Option<String>,
    pub progress: Option<TransferProgress>,
    /// When a `Waiting` job searches again, in ms since the epoch.
    #[serde(default)]
    pub retry_at: Option<u64>,
    pub output_path: Option<PathBuf>,
    /// A lossy library file this download upgrades; moved to the Trash on success.
    #[serde(default)]
    pub replaces: Option<PathBuf>,
    /// Completed without downloading because `output_path` already had this track.
    #[serde(default)]
    pub already_in_library: bool,
    /// A rekordbox playlist to add the track to once it's in the collection.
    #[serde(default)]
    pub playlist: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub source: JobSource,
    /// Set while and after downloading from YouTube Music.
    #[serde(default)]
    pub youtube: Option<YoutubeSource>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Job {
    pub fn new(request: TrackRequest, replaces: Option<PathBuf>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4(),
            request,
            metadata: None,
            status: JobStatus::Queued,
            candidates: Vec::new(),
            searches: Vec::new(),
            active_candidate: None,
            transfer_id: None,
            progress: None,
            retry_at: None,
            output_path: None,
            replaces,
            already_in_library: false,
            playlist: None,
            error: None,
            source: JobSource::Soulseek,
            youtube: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Clear per-attempt state so the job can be run again. Metadata is
    /// resolved afresh too, so matching fixes apply to retries.
    pub fn reset(&mut self) {
        self.status = JobStatus::Queued;
        self.metadata = None;
        self.candidates.clear();
        self.searches.clear();
        self.already_in_library = false;
        self.active_candidate = None;
        self.transfer_id = None;
        self.progress = None;
        self.retry_at = None;
        self.output_path = None;
        self.error = None;
        self.source = JobSource::Soulseek;
        self.youtube = None;
    }
}
