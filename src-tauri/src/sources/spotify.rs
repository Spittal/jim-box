//! Public Spotify playlists, read without an API key via the embed widget's
//! page data. The embed only includes the first 100 tracks.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const EMBED: &str = "https://open.spotify.com/embed/playlist";
/// The most tracks the embed page lists.
pub const EMBED_TRACK_LIMIT: usize = 100;

pub struct Spotify {
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyPlaylist {
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<SpotifyTrack>,
    /// The playlist may have more tracks than the embed shows.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyTrack {
    pub title: String,
    /// All credited artists, lead first.
    pub artists: Vec<String>,
    pub duration_secs: u32,
}

#[derive(Deserialize)]
struct NextData {
    props: Props,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Props {
    page_props: PageProps,
}

#[derive(Deserialize)]
struct PageProps {
    state: Option<PageState>,
}

#[derive(Deserialize)]
struct PageState {
    data: PageData,
}

#[derive(Deserialize)]
struct PageData {
    entity: Entity,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entity {
    id: String,
    name: String,
    cover_art: Option<CoverArt>,
    #[serde(default)]
    track_list: Vec<EmbedTrack>,
}

#[derive(Deserialize)]
struct CoverArt {
    #[serde(default)]
    sources: Vec<ImageSource>,
}

#[derive(Deserialize)]
struct ImageSource {
    url: String,
}

#[derive(Deserialize)]
struct EmbedTrack {
    title: String,
    /// Artists joined by ",\u{a0}".
    subtitle: String,
    /// Milliseconds.
    duration: u64,
}

impl Spotify {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn playlist(&self, input: &str) -> AppResult<SpotifyPlaylist> {
        let id = parse_playlist_id(input)
            .ok_or_else(|| AppError::Validation("That doesn't look like a Spotify playlist link".into()))?;
        let html = self
            .http
            .get(format!("{EMBED}/{id}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_embed(&html)
    }
}

/// Accepts `https://open.spotify.com/playlist/<id>?si=…` (with or without a
/// locale segment like `/intl-de/`), `spotify:playlist:<id>`, or a bare id.
fn parse_playlist_id(input: &str) -> Option<String> {
    let input = input.trim();
    let id = if let Some(rest) = input.strip_prefix("spotify:playlist:") {
        rest
    } else if input.contains("open.spotify.com") {
        let (_, after) = input.split_once("/playlist/")?;
        after.split(['?', '/', '#']).next()?
    } else {
        input
    };
    let valid = id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then(|| id.to_string())
}

fn parse_embed(html: &str) -> AppResult<SpotifyPlaylist> {
    const START: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let unreadable = || AppError::Validation("Couldn't read that playlist from Spotify".into());

    let json = html
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once("</script>"))
        .map(|(json, _)| json)
        .ok_or_else(unreadable)?;
    let data: NextData = serde_json::from_str(json).map_err(|_| unreadable())?;
    let entity = data
        .props
        .page_props
        .state
        .ok_or_else(|| AppError::Validation("Playlist not found. Is it public?".into()))?
        .data
        .entity;

    let tracks: Vec<SpotifyTrack> = entity
        .track_list
        .into_iter()
        .map(|t| SpotifyTrack {
            title: t.title,
            artists: t
                .subtitle
                .split(',')
                .map(|a| a.trim_matches(|c: char| c.is_whitespace()).to_string())
                .filter(|a| !a.is_empty())
                .collect(),
            duration_secs: ((t.duration + 500) / 1000) as u32,
        })
        .collect();

    Ok(SpotifyPlaylist {
        id: entity.id,
        name: entity.name,
        cover_url: entity.cover_art.and_then(|c| c.sources.into_iter().next()).map(|s| s.url),
        truncated: tracks.len() >= EMBED_TRACK_LIMIT,
        tracks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "37i9dQZF1DXcBWIGoYBM5M";

    #[test]
    fn parses_playlist_ids() {
        for input in [
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc123",
            "https://open.spotify.com/intl-de/playlist/37i9dQZF1DXcBWIGoYBM5M",
            "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
            " 37i9dQZF1DXcBWIGoYBM5M ",
        ] {
            assert_eq!(parse_playlist_id(input).as_deref(), Some(ID), "{input}");
        }
        assert_eq!(parse_playlist_id("https://open.spotify.com/album/37i9dQZF1DXcBWIGoYBM5M"), None);
        assert_eq!(parse_playlist_id("daft punk"), None);
    }

    #[test]
    fn parses_embed_page() {
        let html = format!(
            r#"<html><script id="__NEXT_DATA__" type="application/json">{}</script></html>"#,
            serde_json::json!({ "props": { "pageProps": { "state": { "data": { "entity": {
                "id": ID,
                "name": "Mix",
                "coverArt": { "sources": [{ "url": "https://i.scdn.co/x" }] },
                "trackList": [
                    { "title": "Dai Dai", "subtitle": "Shakira,\u{a0}Burna Boy", "duration": 202460 }
                ]
            }}}}}})
        );
        let playlist = parse_embed(&html).unwrap();
        assert_eq!(playlist.name, "Mix");
        assert!(!playlist.truncated);
        let track = &playlist.tracks[0];
        assert_eq!(track.artists, vec!["Shakira", "Burna Boy"]);
        assert_eq!(track.duration_secs, 202);
    }

    #[test]
    fn reports_missing_playlists() {
        let html = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"status":404}}}</script>"#;
        assert!(parse_embed(html).unwrap_err().to_string().contains("not found"));
    }
}
