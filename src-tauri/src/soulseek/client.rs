//! Searches and downloads over a logged-in Soulseek connection. The client
//! library is synchronous and keeps its own threads; this is the async face
//! the jobs use.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use soulseek_rs::{Client, SessionLoss};
use tracing::warn;

use super::types::{SearchResponse, Transfer};
use crate::error::{AppError, AppResult};

/// A search ends once no peer has answered for this long. Each answer
/// restarts the clock, so searches usually run longer.
const SEARCH_QUIET: Duration = Duration::from_secs(15);
/// Searches still collecting after this stop, keeping what arrived.
const SEARCH_MAX_DURATION: Duration = Duration::from_secs(45);
/// Enough peers to choose from; the search stops there.
const SEARCH_RESPONSE_LIMIT: usize = 200;
/// Minimum time between starting searches. The Soulseek server is suspected
/// to drop searches from clients that send them in bursts.
const SEARCH_GAP: Duration = Duration::from_secs(2);
const SEARCH_POLL: Duration = Duration::from_millis(500);

pub struct SoulseekClient {
    /// There until dropped.
    inner: Option<Client>,
    /// Set when the connection is being replaced or shut down, so whatever
    /// is still using it lets go.
    closed: AtomicBool,
    /// When the last search started; searches are started one at a time.
    last_search: tokio::sync::Mutex<Option<Instant>>,
    /// The library files answers under the search's text, so two searches
    /// for the same text can't run at once.
    searching: Mutex<HashSet<String>>,
}

impl SoulseekClient {
    /// Wraps a client that has connected and logged in.
    pub fn new(client: Client) -> Self {
        Self {
            inner: Some(client),
            closed: AtomicBool::new(false),
            last_search: tokio::sync::Mutex::new(None),
            searching: Mutex::new(HashSet::new()),
        }
    }

    fn inner(&self) -> &Client {
        self.inner.as_ref().expect("client is there until dropped")
    }

    /// Why the server connection ended, if it has.
    pub fn session_loss(&self) -> Option<SessionLoss> {
        self.inner().session_loss()
    }

    /// Makes everything still using this connection give up on it.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    fn check_live(&self) -> AppResult<()> {
        if self.closed.load(Ordering::Relaxed) || self.session_loss().is_some() {
            return Err(AppError::SoulseekNotReady);
        }
        Ok(())
    }

    /// Runs a network-wide search and returns every peer's answer.
    pub async fn search(self: &Arc<Self>, text: &str) -> AppResult<Vec<SearchResponse>> {
        let _claim = SearchClaim::take(self, text).await;
        {
            let mut last = self.last_search.lock().await;
            if let Some(wait) = last.and_then(|t| SEARCH_GAP.checked_sub(t.elapsed())) {
                tokio::time::sleep(wait).await;
            }
            self.check_live()?;
            *last = Some(Instant::now());
            // With no time to collect, this only sends the search.
            self.inner().search_with_cancel(text, Duration::ZERO, None)?;
        }

        // Peers answer whenever they get round to it; nothing says a search
        // is over.
        let started = Instant::now();
        let mut answered = 0;
        let mut last_answer = started;
        loop {
            tokio::time::sleep(SEARCH_POLL).await;
            self.check_live()?;
            let count = self.inner().get_search_results_count(text);
            if count != answered {
                answered = count;
                last_answer = Instant::now();
            }
            if answered >= SEARCH_RESPONSE_LIMIT
                || last_answer.elapsed() > SEARCH_QUIET
                || started.elapsed() > SEARCH_MAX_DURATION
            {
                break;
            }
        }

        // Copies every file of every answer.
        let (client, query) = (self.clone(), text.to_string());
        let results = blocking(move || client.inner().get_search_results(&query)).await?;
        Ok(results.into_iter().map(SearchResponse::from).collect())
    }

    /// Asks the peer for a file, to be saved in `dir` under its own name.
    pub async fn enqueue(self: &Arc<Self>, username: &str, filename: &str, size: u64, dir: &Path) -> AppResult<Transfer> {
        self.check_live()?;
        // Found out here rather than after the wait in the peer's queue.
        if !can_save(filename) {
            return Err(AppError::Soulseek(UNSAVEABLE_NAME.into()));
        }
        let client = self.clone();
        let (username, filename, dir) = (username.to_string(), filename.to_string(), dir.to_string_lossy().into_owned());
        // The status channel goes unread: the transfer is followed by polling.
        let (download, _) = blocking(move || client.inner().download(filename, username, size, dir)).await??;
        Ok(Transfer::from(&download))
    }

    /// Where the download of a peer's file stands. It's found by peer and
    /// file name: the token it was queued under is swapped for the peer's own
    /// once the peer starts sending.
    pub fn transfer(&self, username: &str, filename: &str) -> AppResult<Transfer> {
        self.check_live()?;
        self.inner()
            .get_all_downloads()
            .iter()
            .filter(|download| download.username == username && download.filename == filename)
            // One still running counts over a finished one of the same file.
            .min_by_key(|download| download.is_finished())
            .map(Transfer::from)
            .ok_or_else(|| AppError::Soulseek("The transfer is gone".into()))
    }

    /// Asks the peer where a queued download stands; the answer shows up as
    /// the transfer's place in the queue.
    pub fn request_place_in_queue(&self, username: &str, filename: &str) {
        let _ = self.inner().request_place_in_queue(username, filename);
    }

    /// Stops a download, discarding what it fetched, and forgets it.
    pub fn cancel(&self, username: &str, filename: &str) {
        let _ = self.inner().cancel_download(username, filename);
        self.forget(username, filename);
    }

    /// Drops a finished download from the client's list.
    pub fn forget(&self, username: &str, filename: &str) {
        let _ = self.inner().remove_download(username, filename);
    }

    /// Replaces the shared folders, which are scanned again.
    pub async fn set_shares(self: &Arc<Self>, dirs: Vec<PathBuf>) -> AppResult<()> {
        self.check_live()?;
        let client = self.clone();
        blocking(move || client.inner().set_shared_directories(share_names(&dirs))).await??;
        Ok(())
    }
}

impl Drop for SoulseekClient {
    fn drop(&mut self) {
        // Disconnecting waits on the client's threads; whoever let go last
        // needn't.
        if let Some(client) = self.inner.take() {
            std::thread::spawn(move || drop(client));
        }
    }
}

/// Holds a search's text for as long as it runs, then drops what it collected.
struct SearchClaim {
    client: Arc<SoulseekClient>,
    text: String,
}

impl SearchClaim {
    async fn take(client: &Arc<SoulseekClient>, text: &str) -> Self {
        while !client.searching.lock().expect("searches lock poisoned").insert(text.to_string()) {
            tokio::time::sleep(SEARCH_POLL).await;
        }
        Self { client: client.clone(), text: text.to_string() }
    }
}

impl Drop for SearchClaim {
    fn drop(&mut self) {
        let _ = self.client.inner().forget_search(&self.text);
        self.client.searching.lock().expect("searches lock poisoned").remove(&self.text);
    }
}

pub const UNSAVEABLE_NAME: &str = "The file's name has characters Windows doesn't allow in file names";

/// Whether a peer's file can be saved on this computer. The client library
/// saves a download under the name the peer gave it, as it is, and a peer on
/// macOS or Linux may use names Windows refuses.
pub fn can_save(filename: &str) -> bool {
    !cfg!(windows) || windows_allows(filename.rsplit(['\\', '/']).next().unwrap_or(filename))
}

fn windows_allows(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    // A device name is one whatever its extension.
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    !name.is_empty()
        && !name.chars().any(|c| c.is_control() || r#"<>:"|?*"#.contains(c))
        && !RESERVED.iter().any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

pub(super) fn share_names(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter().map(|dir| dir.to_string_lossy().into_owned()).collect()
}

/// Runs one of the client library's blocking calls off the async threads.
pub(super) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        warn!("Soulseek task failed: {e}");
        AppError::Soulseek("an internal task failed".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_names_windows_refuses() {
        assert!(windows_allows("01 One More Time.flac"));
        assert!(windows_allows("Señor (Tales of Yankee Power) [live].flac"));
        assert!(!windows_allows("01 Who Is It?.flac"));
        assert!(!windows_allows("Ratio 16:9.flac"));
        assert!(!windows_allows("A \"B\" C.flac"));
        assert!(!windows_allows("a*b.flac"));
        assert!(!windows_allows("con.flac"));
        assert!(windows_allows("Contact.flac"));
        assert!(!windows_allows(""));
    }
}
