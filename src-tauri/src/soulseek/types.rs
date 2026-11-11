//! What the rest of the app sees of Soulseek: search responses and transfers,
//! independent of the client library's own types.

use soulseek_rs::types::{Download, DownloadStatus};
use soulseek_rs::SearchResult;

/// File attribute codes peers attach to search results.
const ATTR_BIT_RATE: u32 = 0;
const ATTR_LENGTH: u32 = 1;
const ATTR_SAMPLE_RATE: u32 = 4;
const ATTR_BIT_DEPTH: u32 = 5;

/// One peer's answer to a search.
#[derive(Debug, Clone)]
pub struct SearchResponse {
    pub username: String,
    pub files: Vec<SearchFile>,
    pub has_free_upload_slot: bool,
    /// Always 0: the client library doesn't report the peer's queue length.
    pub queue_length: u64,
    /// Bytes per second.
    pub upload_speed: u64,
}

#[derive(Debug, Clone)]
pub struct SearchFile {
    /// The peer's full path to the file, backslash-separated.
    pub filename: String,
    pub size: u64,
    pub bit_depth: Option<u32>,
    /// Kilobits per second.
    pub bit_rate: Option<u32>,
    pub sample_rate: Option<u32>,
    /// Seconds.
    pub length: Option<u32>,
}

impl From<SearchResult> for SearchResponse {
    fn from(result: SearchResult) -> Self {
        Self {
            username: result.username,
            has_free_upload_slot: result.slots > 0,
            queue_length: 0,
            upload_speed: u64::from(result.speed),
            files: result
                .files
                .into_iter()
                .map(|file| SearchFile {
                    bit_depth: file.attribs.get(&ATTR_BIT_DEPTH).copied(),
                    bit_rate: file.attribs.get(&ATTR_BIT_RATE).copied(),
                    sample_rate: file.attribs.get(&ATTR_SAMPLE_RATE).copied(),
                    length: file.attribs.get(&ATTR_LENGTH).copied(),
                    filename: file.name,
                    size: file.size,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferPhase {
    /// Waiting to reach the peer, or in its upload queue.
    Queued,
    InProgress,
    Succeeded,
    Failed(String),
}

/// A download's state at one moment.
#[derive(Debug, Clone)]
pub struct Transfer {
    /// Identifies the download for the life of the connection.
    pub token: u32,
    pub size: u64,
    pub bytes_transferred: u64,
    /// Bytes per second.
    pub speed: f64,
    pub place_in_queue: Option<u32>,
    pub phase: TransferPhase,
}

impl From<&Download> for Transfer {
    fn from(download: &Download) -> Self {
        Self {
            token: download.token,
            size: download.size,
            bytes_transferred: download.bytes_downloaded(),
            speed: download.speed_bytes_per_sec(),
            place_in_queue: download.queue_position.filter(|_| matches!(download.status, DownloadStatus::Queued)),
            phase: phase(&download.status),
        }
    }
}

fn phase(status: &DownloadStatus) -> TransferPhase {
    match status {
        DownloadStatus::Queued | DownloadStatus::Paused { .. } => TransferPhase::Queued,
        DownloadStatus::InProgress { .. } => TransferPhase::InProgress,
        DownloadStatus::Completed => TransferPhase::Succeeded,
        DownloadStatus::Cancelled => TransferPhase::Failed("Cancelled".into()),
        DownloadStatus::TimedOut => TransferPhase::Failed("Timed out".into()),
        DownloadStatus::Failed(reason) => {
            TransferPhase::Failed(reason.clone().unwrap_or_else(|| "The peer ended the transfer".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soulseek_rs::File;
    use std::collections::HashMap;

    #[test]
    fn maps_a_search_result() {
        let result = SearchResult {
            token: 1,
            slots: 1,
            speed: 250_000,
            username: "peer".into(),
            files: vec![File {
                username: "peer".into(),
                name: r"@@abc\Music\x.flac".into(),
                size: 42,
                attribs: HashMap::from([(1, 180), (4, 44_100), (5, 16)]),
            }],
        };
        let response = SearchResponse::from(result);
        assert!(response.has_free_upload_slot);
        assert_eq!(response.upload_speed, 250_000);
        let file = &response.files[0];
        assert_eq!(file.filename, r"@@abc\Music\x.flac");
        assert_eq!((file.length, file.sample_rate, file.bit_depth, file.bit_rate), (Some(180), Some(44_100), Some(16), None));
    }

    #[test]
    fn maps_download_statuses_to_phases() {
        assert_eq!(phase(&DownloadStatus::Queued), TransferPhase::Queued);
        let in_progress = DownloadStatus::InProgress { bytes_downloaded: 1, total_bytes: 2, speed_bytes_per_sec: 1.0 };
        assert_eq!(phase(&in_progress), TransferPhase::InProgress);
        assert_eq!(phase(&DownloadStatus::Completed), TransferPhase::Succeeded);
        assert_eq!(phase(&DownloadStatus::Failed(Some("Denied".into()))), TransferPhase::Failed("Denied".into()));
        assert_eq!(phase(&DownloadStatus::TimedOut), TransferPhase::Failed("Timed out".into()));
    }
}
