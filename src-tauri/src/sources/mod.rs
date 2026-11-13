//! Clients for the outside services Jim Box reads from. They return plain
//! data; deciding what matches what is `autotag`'s job.

mod deezer;
pub mod musicbrainz;
mod spotify;
pub mod stores;
pub mod youtube;

use tracing::warn;

pub use deezer::Deezer;
pub use musicbrainz::MusicBrainz;
pub use spotify::{Spotify, SpotifyTrack};
pub use stores::Stores;

/// Identifies HTTP clients to MusicBrainz, which requires a meaningful User-Agent.
pub const USER_AGENT: &str = concat!("jim-box/", env!("CARGO_PKG_VERSION"), " ( desktop app )");

/// Deezer for search, MusicBrainz for canonical tags, Spotify for reading
/// public playlists, stores for buying lossless copies, and artwork downloads.
pub struct Sources {
    pub deezer: Deezer,
    pub musicbrainz: MusicBrainz,
    pub spotify: Spotify,
    pub stores: Stores,
    http: reqwest::Client,
}

impl Sources {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build HTTP client");
        Self {
            deezer: Deezer::new(http.clone()),
            musicbrainz: MusicBrainz::new(http.clone()),
            spotify: Spotify::new(http.clone()),
            stores: Stores::new(http.clone()),
            http,
        }
    }

    /// Downloads the first available artwork from `urls`.
    pub async fn fetch_cover(&self, urls: &[String]) -> Option<Vec<u8>> {
        for url in urls {
            let res = match self.http.get(url).send().await.and_then(|r| r.error_for_status()) {
                Ok(res) => res,
                Err(e) => {
                    // 404 is normal: not every release has artwork.
                    if e.status() != Some(reqwest::StatusCode::NOT_FOUND) {
                        warn!("Cover art fetch failed for {url}: {e}");
                    }
                    continue;
                }
            };
            match res.bytes().await {
                Ok(bytes) => return Some(bytes.to_vec()),
                Err(e) => warn!("Cover art download failed for {url}: {e}"),
            }
        }
        None
    }
}
