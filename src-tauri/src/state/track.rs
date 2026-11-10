use serde::{Deserialize, Serialize};

/// A track the user picked from search results. This is what they asked for;
/// [`TrackMetadata`] is what we resolve it to for tagging. Comes from Deezer,
/// or from MusicBrainz for tracks Deezer doesn't carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRequest {
    #[serde(default)]
    pub deezer_id: Option<u64>,
    #[serde(default)]
    pub mb_recording_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub isrc: Option<String>,
    pub duration_secs: u32,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub release_date: Option<String>,
    /// Small artwork for display.
    pub cover_url: Option<String>,
    /// Large artwork, used as a tagging fallback when MusicBrainz has none.
    pub cover_xl_url: Option<String>,
    /// Deezer's 30-second clip. The link stops working after about 15 minutes.
    #[serde(default)]
    pub preview_url: Option<String>,
}

/// Canonical metadata written into the file's tags and used for its library path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// The album artist without collaborators ("Fred again.." for "Fred
    /// again.., Skrillex & Four Tet"), used for library folders.
    #[serde(default)]
    pub primary_album_artist: Option<String>,
    /// A Various Artists compilation (beets' `comp`), filed by the
    /// compilation path template.
    #[serde(default)]
    pub compilation: bool,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    /// Release date as `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    pub date: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<u32>,
    pub mb_recording_id: Option<String>,
    pub mb_release_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_artist_id: Option<String>,
    pub mb_album_artist_id: Option<String>,
    /// Artwork URLs to try in order.
    pub cover_art_urls: Vec<String>,
}

impl TrackMetadata {
    pub fn year(&self) -> Option<&str> {
        self.date.as_deref().and_then(|d| d.get(..4))
    }

    /// Metadata derived purely from the search result, used when MusicBrainz
    /// has no match for the track.
    pub fn from_request(request: &TrackRequest) -> Self {
        Self {
            title: request.title.clone(),
            artist: request.artist.clone(),
            album: request.album.clone(),
            album_artist: request.artist.clone(),
            track_number: request.track_number,
            disc_number: request.disc_number,
            date: request.release_date.clone(),
            isrc: request.isrc.clone(),
            duration_ms: Some(request.duration_secs * 1000),
            cover_art_urls: request.cover_xl_url.iter().cloned().collect(),
            ..Default::default()
        }
    }
}
