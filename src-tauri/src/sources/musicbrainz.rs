//! MusicBrainz API client: the source of truth for tags, as in beets. Returns
//! plain data (recordings with every release they appear on, releases with
//! their tracklists); choosing between them is `autotag`'s job.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::state::TrackMetadata;

const API: &str = "https://musicbrainz.org/ws/2";
const COVER_ART_ARCHIVE: &str = "https://coverartarchive.org";
/// MusicBrainz allows one request per second per client.
const MIN_INTERVAL: Duration = Duration::from_millis(1100);
/// MusicBrainz answers 503 when busy or rate limiting; retry this many times.
const BUSY_RETRIES: u32 = 3;
const BUSY_BACKOFF: Duration = Duration::from_secs(2);
/// MusicBrainz's "Various Artists" artist.
const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";
const SEARCH_LIMIT: &str = "10";
/// Genres to offer for a track: past the first few, they're one person's vote.
const MAX_GENRES: usize = 6;

// --- Domain types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseKind {
    Album,
    Ep,
    Single,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStatus {
    Official,
    Bootleg,
    Other,
}

/// A release without its tracklist.
#[derive(Debug, Clone)]
pub struct ReleaseSummary {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    /// Total tracks across media, when known.
    pub track_count: Option<u32>,
    pub status: ReleaseStatus,
    pub kind: ReleaseKind,
    /// A compilation, soundtrack, live album… or credited to Various Artists.
    pub compilation: bool,
    pub various_artists: bool,
    /// CD or digital, whose track numbering runs per disc rather than per side.
    pub preferred_media: bool,
}

/// A recording, with its metadata on each release it appears on.
#[derive(Debug, Clone)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub length_ms: Option<u32>,
    /// Track-level metadata without any release.
    pub base: TrackMetadata,
    pub appearances: Vec<Appearance>,
}

#[derive(Debug, Clone)]
pub struct Appearance {
    pub release: ReleaseSummary,
    /// The recording's full metadata on this release (album, track number…).
    pub metadata: TrackMetadata,
}

/// One of the recordings an ISRC points to.
#[derive(Debug, Clone)]
pub struct RecordingRef {
    pub id: String,
    pub title: String,
    pub length_ms: Option<u32>,
}

/// A release with its full tracklist, each track ready to tag with.
#[derive(Debug, Clone)]
pub struct AlbumRelease {
    pub summary: ReleaseSummary,
    pub tracks: Vec<AlbumTrack>,
}

#[derive(Debug, Clone)]
pub struct AlbumTrack {
    pub disc: u32,
    pub position: u32,
    pub title: String,
    pub length_secs: Option<u32>,
    pub metadata: TrackMetadata,
}

/// Where a release group can be bought, per MusicBrainz.
#[derive(Debug, Clone)]
pub struct PurchaseLinks {
    pub urls: Vec<String>,
    /// The release group is a single rather than an album or EP.
    pub single: bool,
}

// --- Client ---

pub struct MusicBrainz {
    http: reqwest::Client,
    last_request: Mutex<Option<Instant>>,
}

impl MusicBrainz {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, last_request: Mutex::new(None) }
    }

    /// A recording and every release it appears on.
    pub async fn recording(&self, id: &str) -> AppResult<Option<Recording>> {
        let recording = self
            .get::<RecordingDto>(&format!("/recording/{id}"), &[("inc", "releases release-groups media artist-credits isrcs")])
            .await;
        Ok(not_found_as_none(recording)?.map(into_recording))
    }

    /// The genres MusicBrainz's users have voted onto a recording, the albums
    /// it's on and its artists: the most specific first, and of those the
    /// most voted. Empty when nobody has, which is common for recordings.
    pub async fn genres(&self, recording_id: &str) -> AppResult<Vec<String>> {
        let found = self
            .get::<GenreLookup>(
                &format!("/recording/{recording_id}"),
                &[("inc", "genres artist-credits releases release-groups")],
            )
            .await;
        Ok(not_found_as_none(found)?.map(rank_genres).unwrap_or_default())
    }

    /// The recordings an ISRC is assigned to (usually one).
    pub async fn isrc_recordings(&self, isrc: &str) -> AppResult<Vec<RecordingRef>> {
        let lookup = not_found_as_none(self.get::<IsrcLookup>(&format!("/isrc/{isrc}"), &[]).await)?;
        Ok(lookup
            .map(|l| {
                l.recordings
                    .into_iter()
                    .map(|r| RecordingRef { id: r.id, title: r.title, length_ms: r.length })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Recordings by exact title and artist.
    pub async fn search_recordings(&self, title: &str, artist: &str) -> AppResult<Vec<Recording>> {
        let query = format!("recording:{} AND artist:{}", lucene_phrase(title), lucene_phrase(artist));
        self.recording_search(&[("query", &query), ("limit", SEARCH_LIMIT)]).await
    }

    /// Recordings matching free text like "hundred waters wave to anchor".
    /// Good for obscure tracks, noisy (covers, tributes) for popular ones.
    pub async fn search_text(&self, text: &str) -> AppResult<Vec<Recording>> {
        self.recording_search(&[("query", text), ("dismax", "true"), ("limit", SEARCH_LIMIT)]).await
    }

    /// Releases matching an album title and artist. The title's words are
    /// matched loosely, so "str011 soar" finds "soar".
    pub async fn search_releases(&self, album: &str, artist: &str) -> AppResult<Vec<ReleaseSummary>> {
        let words = lucene_words(album);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = if artist.is_empty() {
            format!("release:({words})")
        } else {
            format!("release:({words}) AND artist:{}", lucene_phrase(artist))
        };
        let found: ReleaseSearch = self.get("/release", &[("query", &query), ("limit", SEARCH_LIMIT)]).await?;
        Ok(found.releases.iter().map(summarize).collect())
    }

    /// A release with its full tracklist.
    pub async fn release(&self, id: &str) -> AppResult<Option<AlbumRelease>> {
        let release = self
            .get::<Release>(&format!("/release/{id}"), &[("inc", "recordings artist-credits release-groups isrcs")])
            .await;
        Ok(not_found_as_none(release)?.map(|r| album_release(&r)))
    }

    /// "Purchase for download" links on any release in a release group, in
    /// MusicBrainz's order and without duplicates.
    pub async fn purchase_links(&self, release_group_id: &str) -> AppResult<PurchaseLinks> {
        let found: ReleaseBrowse = self
            .get(
                "/release",
                &[("release-group", release_group_id), ("inc", "url-rels release-groups"), ("limit", "100")],
            )
            .await?;
        let single = found
            .releases
            .first()
            .and_then(|r| r.release_group.as_ref())
            .is_some_and(|g| g.primary_type.as_deref() == Some("Single"));
        let mut urls: Vec<String> = Vec::new();
        let relations = found.releases.into_iter().flat_map(|r| r.relations);
        for url in relations.filter(|r| r.kind == "purchase for download").filter_map(|r| r.url) {
            if !urls.contains(&url.resource) {
                urls.push(url.resource);
            }
        }
        Ok(PurchaseLinks { urls, single })
    }

    async fn recording_search(&self, query: &[(&str, &str)]) -> AppResult<Vec<Recording>> {
        let found: RecordingSearch = self.get("/recording", query).await?;
        Ok(found.recordings.into_iter().map(into_recording).collect())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> AppResult<T> {
        let mut attempt = 0;
        loop {
            self.throttle().await;
            let res = self
                .http
                .get(format!("{API}{path}"))
                .query(&[("fmt", "json")])
                .query(query)
                .send()
                .await?;
            if res.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE && attempt < BUSY_RETRIES {
                attempt += 1;
                tokio::time::sleep(BUSY_BACKOFF * attempt).await;
                continue;
            }
            return Ok(res.error_for_status()?.json().await?);
        }
    }

    /// Waits until a request is allowed.
    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(elapsed) = last.map(|t| t.elapsed()) {
            if elapsed < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }
}

// --- API responses ---

#[derive(Deserialize)]
struct IsrcLookup {
    #[serde(default)]
    recordings: Vec<IsrcRecording>,
}

#[derive(Deserialize)]
struct IsrcRecording {
    id: String,
    title: String,
    length: Option<u32>,
}

#[derive(Deserialize)]
struct RecordingSearch {
    #[serde(default)]
    recordings: Vec<RecordingDto>,
}

#[derive(Deserialize)]
struct ReleaseSearch {
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Deserialize)]
struct ReleaseBrowse {
    #[serde(default)]
    releases: Vec<ReleaseWithUrls>,
}

#[derive(Deserialize)]
struct ReleaseWithUrls {
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroup>,
    #[serde(default)]
    relations: Vec<UrlRelation>,
}

#[derive(Deserialize)]
struct UrlRelation {
    #[serde(rename = "type")]
    kind: String,
    url: Option<UrlResource>,
}

#[derive(Deserialize)]
struct UrlResource {
    resource: String,
}

/// A recording with the genres voted onto it and onto what it's part of.
#[derive(Deserialize)]
struct GenreLookup {
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<GenreCredit>,
    #[serde(default)]
    releases: Vec<GenreRelease>,
}

#[derive(Deserialize)]
struct Genre {
    name: String,
    #[serde(default)]
    count: i32,
}

#[derive(Deserialize)]
struct GenreCredit {
    artist: Tagged,
}

#[derive(Deserialize)]
struct GenreRelease {
    #[serde(rename = "release-group")]
    release_group: Option<Tagged>,
}

/// An artist or release group, for its genres.
#[derive(Deserialize)]
struct Tagged {
    id: String,
    #[serde(default)]
    genres: Vec<Genre>,
    /// Of a release group: what makes it a compilation, a soundtrack…
    #[serde(rename = "secondary-types", default)]
    secondary_types: Vec<String>,
}

#[derive(Deserialize)]
struct RecordingDto {
    id: String,
    title: String,
    length: Option<u32>,
    #[serde(default)]
    isrcs: Vec<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Clone, Deserialize)]
struct ArtistCredit {
    name: String,
    #[serde(default)]
    joinphrase: String,
    artist: ArtistRef,
}

#[derive(Debug, Clone, Deserialize)]
struct ArtistRef {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    id: String,
    title: String,
    status: Option<String>,
    date: Option<String>,
    /// Only in search results.
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroup>,
    #[serde(default)]
    media: Vec<Medium>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseGroup {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "secondary-types", default)]
    secondary_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Medium {
    position: Option<u32>,
    format: Option<String>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
    #[serde(default)]
    tracks: Vec<MediumTrack>,
}

/// A track on a medium. Recording lookups only include `position`; release
/// lookups include the rest.
#[derive(Debug, Clone, Deserialize)]
struct MediumTrack {
    position: Option<u32>,
    #[serde(default)]
    title: String,
    length: Option<u32>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    recording: Option<TrackRecording>,
}

#[derive(Debug, Clone, Deserialize)]
struct TrackRecording {
    id: String,
    #[serde(default)]
    isrcs: Vec<String>,
}

// --- Conversions ---

fn into_recording(dto: RecordingDto) -> Recording {
    let artist = credit_name(&dto.artist_credit);
    let base = TrackMetadata {
        title: dto.title.clone(),
        artist: artist.clone(),
        album_artist: artist,
        primary_album_artist: dto.artist_credit.first().map(|c| c.name.clone()),
        isrc: dto.isrcs.first().cloned(),
        duration_ms: dto.length,
        mb_recording_id: Some(dto.id.clone()),
        mb_artist_id: dto.artist_credit.first().map(|c| c.artist.id.clone()),
        ..Default::default()
    };
    let appearances = dto
        .releases
        .iter()
        .map(|release| {
            let mut metadata = base.clone();
            let medium = release.media.first();
            apply_release(&mut metadata, release, medium, medium.and_then(|m| m.tracks.first()));
            Appearance { release: summarize(release), metadata }
        })
        .collect();
    Recording { id: dto.id, title: dto.title, length_ms: dto.length, base, appearances }
}

/// The recording's own genres, then its albums', then its artists'; each by
/// votes. Compilations are left out: their genres are of the other tracks.
fn rank_genres(found: GenreLookup) -> Vec<String> {
    let mut groups: Vec<Tagged> = Vec::new();
    for group in found.releases.into_iter().filter_map(|r| r.release_group) {
        if group.secondary_types.is_empty() && !groups.iter().any(|g| g.id == group.id) {
            groups.push(group);
        }
    }
    let artists = found.artist_credit.into_iter().map(|c| c.artist);
    let levels = [
        found.genres,
        groups.into_iter().flat_map(|g| g.genres).collect(),
        artists.flat_map(|a| a.genres).collect(),
    ];

    let mut names: Vec<String> = Vec::new();
    for level in levels {
        // A genre voted onto two of a recording's albums has both's votes.
        let mut votes: Vec<(String, i32)> = Vec::new();
        for genre in level {
            match votes.iter_mut().find(|(name, _)| *name == genre.name) {
                Some((_, count)) => *count += genre.count,
                None => votes.push((genre.name, genre.count)),
            }
        }
        votes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (name, _) in votes {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.truncate(MAX_GENRES);
    names
}

fn summarize(release: &Release) -> ReleaseSummary {
    let group = release.release_group.as_ref();
    let various_artists = release.artist_credit.first().is_some_and(|c| c.artist.id == VARIOUS_ARTISTS_ID);
    ReleaseSummary {
        id: release.id.clone(),
        title: release.title.clone(),
        artist: credit_name(&release.artist_credit),
        date: release.date.clone().filter(|d| !d.is_empty()),
        track_count: release.track_count.or_else(|| release.media.iter().map(|m| m.track_count).sum()),
        status: match release.status.as_deref() {
            Some("Official") => ReleaseStatus::Official,
            Some("Bootleg") => ReleaseStatus::Bootleg,
            _ => ReleaseStatus::Other,
        },
        kind: match group.and_then(|g| g.primary_type.as_deref()) {
            Some("Album") => ReleaseKind::Album,
            Some("EP") => ReleaseKind::Ep,
            Some("Single") => ReleaseKind::Single,
            _ => ReleaseKind::Other,
        },
        compilation: group.is_some_and(|g| !g.secondary_types.is_empty()) || various_artists,
        various_artists,
        preferred_media: release
            .media
            .first()
            .and_then(|m| m.format.as_deref())
            .is_some_and(|f| f == "CD" || f == "Digital Media"),
    }
}

fn album_release(release: &Release) -> AlbumRelease {
    let tracks = release
        .media
        .iter()
        .flat_map(|medium| medium.tracks.iter().map(move |track| (medium, track)))
        .filter_map(|(medium, track)| {
            let recording = track.recording.as_ref()?;
            let credit = if track.artist_credit.is_empty() { &release.artist_credit } else { &track.artist_credit };
            let artist = credit_name(credit);
            let mut metadata = TrackMetadata {
                title: track.title.clone(),
                artist: artist.clone(),
                album_artist: artist,
                isrc: recording.isrcs.first().cloned(),
                duration_ms: track.length,
                mb_recording_id: Some(recording.id.clone()),
                mb_artist_id: credit.first().map(|c| c.artist.id.clone()),
                ..Default::default()
            };
            apply_release(&mut metadata, release, Some(medium), Some(track));
            Some(AlbumTrack {
                disc: medium.position.unwrap_or(1),
                position: track.position?,
                title: track.title.clone(),
                length_secs: track.length.map(|ms| (ms + 500) / 1000),
                metadata,
            })
        })
        .collect();
    AlbumRelease { summary: summarize(release), tracks }
}

/// Fills a track's album fields from the release it's on.
fn apply_release(meta: &mut TrackMetadata, release: &Release, medium: Option<&Medium>, track: Option<&MediumTrack>) {
    meta.album = release.title.clone();
    if !release.artist_credit.is_empty() {
        meta.album_artist = credit_name(&release.artist_credit);
        meta.primary_album_artist = release.artist_credit.first().map(|c| c.name.clone());
        meta.mb_album_artist_id = release.artist_credit.first().map(|c| c.artist.id.clone());
    }
    meta.date = release.date.clone().filter(|d| !d.is_empty());
    meta.compilation = release.artist_credit.first().is_some_and(|c| c.artist.id == VARIOUS_ARTISTS_ID);
    if let Some(medium) = medium {
        meta.disc_number = medium.position;
        meta.track_total = medium.track_count;
    }
    meta.track_number = track.and_then(|t| t.position);
    meta.cover_art_urls.push(format!("{COVER_ART_ARCHIVE}/release/{}/front-1200", release.id));
    if let Some(group) = &release.release_group {
        meta.cover_art_urls.push(format!("{COVER_ART_ARCHIVE}/release-group/{}/front-1200", group.id));
        meta.mb_release_group_id = Some(group.id.clone());
    }
    meta.mb_release_id = Some(release.id.clone());
}

/// Joins a multi-artist credit the way MusicBrainz displays it ("A feat. B").
fn credit_name(credit: &[ArtistCredit]) -> String {
    credit.iter().map(|c| format!("{}{}", c.name, c.joinphrase)).collect()
}

/// Artwork for a release, at a Cover Art Archive thumbnail size (250, 500, 1200).
pub fn cover_url(release_id: &str, size: u32) -> String {
    format!("{COVER_ART_ARCHIVE}/release/{release_id}/front-{size}")
}

/// Quotes a value for a MusicBrainz (Lucene) search query.
fn lucene_phrase(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A value's words for a loose search, without Lucene syntax characters.
fn lucene_words(value: &str) -> String {
    const SPECIAL: &[char] = &['+', '-', '&', '|', '!', '(', ')', '{', '}', '[', ']', '^', '"', '~', '*', '?', ':', '\\', '/'];
    value.replace(SPECIAL, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn not_found_as_none<T>(result: AppResult<T>) -> AppResult<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(AppError::Http(e)) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_artist_credits() {
        let credit = vec![
            ArtistCredit { name: "Daft Punk".into(), joinphrase: " feat. ".into(), artist: ArtistRef { id: "a".into() } },
            ArtistCredit { name: "Pharrell Williams".into(), joinphrase: "".into(), artist: ArtistRef { id: "b".into() } },
        ];
        assert_eq!(credit_name(&credit), "Daft Punk feat. Pharrell Williams");
    }

    #[test]
    fn ranks_genres_from_the_recording_outwards() {
        let found: GenreLookup = serde_json::from_str(
            r#"{
                "genres": [{"name": "french house", "count": 1}],
                "artist-credit": [{"artist": {"id": "a", "genres": [
                    {"name": "electronic", "count": 42}, {"name": "house", "count": 22}, {"name": "trap", "count": 1}
                ]}}],
                "releases": [
                    {"release-group": {"id": "g", "genres": [{"name": "house", "count": 3}, {"name": "disco", "count": 2}]}},
                    {"release-group": {"id": "g", "genres": [{"name": "house", "count": 3}, {"name": "disco", "count": 2}]}},
                    {"release-group": {"id": "h", "genres": [{"name": "disco", "count": 2}]}},
                    {"release-group": {"id": "c", "secondary-types": ["Compilation"], "genres": [{"name": "pop", "count": 9}]}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(rank_genres(found), ["french house", "disco", "house", "electronic", "trap"]);
    }

    #[test]
    fn escapes_search_values() {
        assert_eq!(lucene_phrase(r#"Say "Hi""#), r#""Say \"Hi\"""#);
        assert_eq!(lucene_words("str011: soar (Deluxe)"), "str011 soar Deluxe");
    }
}
