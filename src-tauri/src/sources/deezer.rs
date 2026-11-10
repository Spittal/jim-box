//! Deezer's public API (no key required). Used for search because its ranking
//! reliably surfaces the canonical recording and it exposes ISRCs, which we
//! use to find the exact recording on MusicBrainz.

use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::error::AppResult;
use crate::state::TrackRequest;

const API: &str = "https://api.deezer.com";
/// Deezer allows 50 requests per 5 seconds; stay well under it during library scans.
const MIN_INTERVAL: Duration = Duration::from_millis(150);

pub struct Deezer {
    http: reqwest::Client,
    last_request: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<Track>,
}

#[derive(Deserialize)]
struct Track {
    id: u64,
    title: String,
    isrc: Option<String>,
    duration: u32,
    preview: Option<String>,
    artist: Artist,
    album: Album,
    // Only present on the single-track endpoint.
    track_position: Option<u32>,
    disk_number: Option<u32>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

#[derive(Deserialize)]
struct Album {
    title: String,
    cover_medium: Option<String>,
    cover_xl: Option<String>,
}

impl Deezer {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            last_request: Mutex::new(None),
        }
    }

    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(elapsed) = last.map(|t| t.elapsed()) {
            if elapsed < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    pub async fn search(&self, query: &str) -> AppResult<Vec<TrackRequest>> {
        self.throttle().await;
        let res: SearchResponse = self
            .http
            .get(format!("{API}/search"))
            .query(&[("q", query), ("limit", "25")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(res.data.into_iter().map(Track::into_request).collect())
    }

    /// Full track details, which include track/disc numbers and release date.
    pub async fn track(&self, id: u64) -> AppResult<TrackRequest> {
        self.throttle().await;
        let track: Track = self
            .http
            .get(format!("{API}/track/{id}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(track.into_request())
    }
}

impl Track {
    fn into_request(self) -> TrackRequest {
        TrackRequest {
            deezer_id: Some(self.id),
            mb_recording_id: None,
            title: self.title,
            artist: self.artist.name,
            album: self.album.title,
            isrc: self.isrc.filter(|s| !s.is_empty()),
            duration_secs: self.duration,
            track_number: self.track_position,
            disc_number: self.disk_number,
            release_date: self.release_date.filter(|d| d != "0000-00-00"),
            cover_url: self.album.cover_medium,
            cover_xl_url: self.album.cover_xl,
            preview_url: self.preview.filter(|s| !s.is_empty()),
        }
    }
}
