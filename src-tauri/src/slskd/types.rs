//! Subsets of slskd's API payloads (v0.26). Only fields we use are modelled.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub server: ServerState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub is_logged_in: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest<'a> {
    pub id: &'a str,
    pub search_text: &'a str,
    /// Milliseconds to wait for peer responses.
    pub search_timeout: u32,
    pub response_limit: u32,
    pub file_limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    pub is_complete: bool,
    #[serde(default)]
    pub response_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub username: String,
    #[serde(default)]
    pub files: Vec<SearchFile>,
    pub has_free_upload_slot: bool,
    pub queue_length: u64,
    /// Bytes per second.
    pub upload_speed: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFile {
    pub filename: String,
    pub size: u64,
    pub bit_depth: Option<u32>,
    pub bit_rate: Option<u32>,
    pub sample_rate: Option<u32>,
    /// Seconds.
    pub length: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueBatchRequest<'a> {
    pub username: &'a str,
    pub files: [EnqueueFile<'a>; 1],
    pub options: EnqueueOptions<'a>,
}

#[derive(Debug, Serialize)]
pub struct EnqueueFile<'a> {
    pub filename: &'a str,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct EnqueueOptions<'a> {
    /// Subdirectory of the downloads dir to save into.
    pub destination: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueBatchResponse {
    pub batch: Option<Batch>,
    #[serde(default)]
    pub failures: Vec<EnqueueFailure>,
}

#[derive(Debug, Deserialize)]
pub struct Batch {
    #[serde(default)]
    pub transfers: Vec<Transfer>,
}

#[derive(Debug, Deserialize)]
pub struct EnqueueFailure {
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub size: u64,
    /// Flags enum rendered as a string, e.g. `"Queued, Remotely"` or `"Completed, Succeeded"`.
    pub state: String,
    pub bytes_transferred: u64,
    pub average_speed: f64,
    pub place_in_queue: Option<u32>,
    pub exception: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransferPhase {
    Queued,
    InProgress,
    Succeeded,
    Failed,
}

impl Transfer {
    pub fn phase(&self) -> TransferPhase {
        if self.state.contains("Succeeded") {
            TransferPhase::Succeeded
        } else if self.state.contains("Completed") {
            TransferPhase::Failed
        } else if self.state.contains("InProgress") {
            TransferPhase::InProgress
        } else {
            TransferPhase::Queued
        }
    }

    pub fn failure_reason(&self) -> String {
        self.exception
            .clone()
            .unwrap_or_else(|| self.state.replace("Completed, ", ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(state: &str) -> Transfer {
        Transfer {
            id: "id".into(),
            size: 1,
            state: state.into(),
            bytes_transferred: 0,
            average_speed: 0.0,
            place_in_queue: None,
            exception: None,
        }
    }

    #[test]
    fn parses_transfer_phases() {
        assert_eq!(transfer("Requested").phase(), TransferPhase::Queued);
        assert_eq!(transfer("Queued, Remotely").phase(), TransferPhase::Queued);
        assert_eq!(transfer("InProgress").phase(), TransferPhase::InProgress);
        assert_eq!(transfer("Completed, Succeeded").phase(), TransferPhase::Succeeded);
        assert_eq!(transfer("Completed, Rejected").phase(), TransferPhase::Failed);
        assert_eq!(transfer("Completed, TimedOut").failure_reason(), "TimedOut");
    }
}
