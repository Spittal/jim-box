mod job;
mod library;
mod settings;
mod track;

pub use job::{Candidate, Job, JobSource, JobStatus, SearchAttempt, TransferProgress, YoutubeSource};
pub use library::{
    AlbumProposal, Confidence, ExistingTags, ItemStatus, LibraryItem, LibraryPhase, LibraryProgress, Proposal,
};
pub use settings::{AudioFormat, PathTemplates, Settings, SettingsState};
pub use track::{TrackMetadata, TrackRequest};

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, used for job timestamps.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
