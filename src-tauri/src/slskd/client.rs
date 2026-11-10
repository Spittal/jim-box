use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use super::types::{
    Application, EnqueueBatchRequest, EnqueueBatchResponse, EnqueueFile, EnqueueOptions, Search,
    SearchRequest, SearchResponse, Transfer,
};
use crate::error::{AppError, AppResult};

/// slskd ends a search once no response has arrived for this long. Each
/// response restarts the clock, so searches usually run longer.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Searches still running after this are stopped, keeping what arrived.
const SEARCH_MAX_DURATION: Duration = Duration::from_secs(45);
/// How long to wait for slskd to finish a search we stopped.
const SEARCH_STOP_GRACE: Duration = Duration::from_secs(10);
/// Minimum time between starting searches. slskd allows one search request at
/// a time, and the Soulseek server is suspected to drop searches from clients
/// that send them in bursts.
const SEARCH_GAP: Duration = Duration::from_secs(2);
const SEARCH_START_ATTEMPTS: u32 = 5;

/// Typed client for the local slskd REST API.
pub struct SlskdClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// slskd rejects concurrent enqueue requests with 429, so serialize them.
    enqueue_lock: Mutex<()>,
    /// When the last search started; searches are started one at a time.
    last_search: Mutex<Option<Instant>>,
}

impl SlskdClient {
    pub fn new(port: u16, api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: format!("http://127.0.0.1:{port}/api/v0"),
            api_key,
            enqueue_lock: Mutex::new(()),
            last_search: Mutex::new(None),
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .header("X-API-Key", &self.api_key)
            .timeout(Duration::from_secs(30))
    }

    pub async fn application(&self) -> AppResult<Application> {
        json(self.request(Method::GET, "/application").send().await?).await
    }

    /// Runs a network-wide search and returns every peer response once it completes.
    pub async fn search(&self, text: &str) -> AppResult<Vec<SearchResponse>> {
        let id = Uuid::new_v4().to_string();
        let body = SearchRequest {
            id: &id,
            search_text: text,
            search_timeout: SEARCH_TIMEOUT.as_millis() as u32,
            response_limit: 200,
            file_limit: 10_000,
        };
        self.start_search(&body).await?;

        // slskd only serves a search's responses once it has completed, so a
        // search must never be abandoned while running: that reads as nobody
        // answering. Long searches are stopped, which completes them.
        let started = Instant::now();
        let mut stopped_at: Option<Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let search: Search =
                json(self.request(Method::GET, &format!("/searches/{id}")).send().await?).await?;
            if search.is_complete {
                break;
            }
            match stopped_at {
                None if started.elapsed() > SEARCH_MAX_DURATION => {
                    check(self.request(Method::PUT, &format!("/searches/{id}")).send().await?).await?;
                    stopped_at = Some(Instant::now());
                }
                Some(at) if at.elapsed() > SEARCH_STOP_GRACE => {
                    warn!(
                        "Search '{text}' didn't finish after being stopped; {} responses may be lost",
                        search.response_count
                    );
                    break;
                }
                _ => {}
            }
        }

        let responses = json(
            self.request(Method::GET, &format!("/searches/{id}/responses"))
                .send()
                .await?,
        )
        .await;

        // Searches are kept by slskd until deleted; clean up regardless of outcome.
        let _ = self.request(Method::DELETE, &format!("/searches/{id}")).send().await;

        responses
    }

    /// Starts a search, spacing searches out and retrying when slskd is busy
    /// starting another one (429).
    async fn start_search(&self, body: &SearchRequest<'_>) -> AppResult<()> {
        let mut last = self.last_search.lock().await;
        let mut attempt = 1;
        loop {
            if let Some(wait) = last.and_then(|t| SEARCH_GAP.checked_sub(t.elapsed())) {
                tokio::time::sleep(wait).await;
            }
            *last = Some(Instant::now());
            let res = self.request(Method::POST, "/searches").json(body).send().await?;
            if res.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || attempt == SEARCH_START_ATTEMPTS {
                check(res).await?;
                return Ok(());
            }
            attempt += 1;
        }
    }

    /// Queues a single file for download into `downloads/<destination>/`.
    pub async fn enqueue(
        &self,
        username: &str,
        filename: &str,
        size: u64,
        destination: &str,
    ) -> AppResult<Transfer> {
        let body = EnqueueBatchRequest {
            username,
            files: [EnqueueFile { filename, size }],
            options: EnqueueOptions { destination },
        };

        let _guard = self.enqueue_lock.lock().await;
        let res = self
            .request(Method::POST, "/transfers/downloads/batches")
            .json(&body)
            .send()
            .await?;
        let res: EnqueueBatchResponse = json(res).await?;

        res.batch
            .and_then(|b| b.transfers.into_iter().next())
            .ok_or_else(|| {
                let reason = res
                    .failures
                    .into_iter()
                    .find_map(|f| f.message)
                    .unwrap_or_else(|| "peer did not accept the request".into());
                AppError::Slskd(format!("Could not queue download: {reason}"))
            })
    }

    pub async fn transfer(&self, username: &str, id: &str) -> AppResult<Transfer> {
        let path = format!("/transfers/downloads/{}/{id}", urlencoding::encode(username));
        json(self.request(Method::GET, &path).send().await?).await
    }

    /// Cancels a transfer and removes it from slskd's list.
    pub async fn cancel(&self, username: &str, id: &str) -> AppResult<()> {
        let path = format!(
            "/transfers/downloads/{}/{id}?remove=true",
            urlencoding::encode(username)
        );
        check(self.request(Method::DELETE, &path).send().await?).await?;
        Ok(())
    }
}

/// Turns non-2xx responses into errors that include slskd's message.
async fn check(res: Response) -> AppResult<Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    let detail = if body.trim().is_empty() {
        status.to_string()
    } else {
        format!("{status}: {}", body.trim())
    };
    Err(AppError::Slskd(detail))
}

async fn json<T: DeserializeOwned>(res: Response) -> AppResult<T> {
    Ok(check(res).await?.json().await?)
}
