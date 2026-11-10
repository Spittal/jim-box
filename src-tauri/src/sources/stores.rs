//! Stores selling lossless downloads. Finds the actual album (or, failing
//! that, track) page for a track, so the app only links to things that exist:
//! Bandcamp and Qobuz are searched directly, and MusicBrainz's "purchase for
//! download" links add other stores (Beatport, 7digital…) after a check that
//! the page is still there.

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tracing::warn;

use super::musicbrainz::{MusicBrainz, PurchaseLinks};
use crate::autotag::{artist_distance, string_distance};
use crate::error::AppResult;
use crate::text::normalize;

const BANDCAMP_SEARCH: &str = "https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic";
const QOBUZ: &str = "https://www.qobuz.com";
/// Qobuz's catalogue differs by country; used when the local store can't be detected.
const QOBUZ_DEFAULT_LOCALE: &str = "us-en";

/// Stores with lossless downloads, by host, in the order they're listed.
/// iTunes and Amazon sell lossy files, so their links are left out.
const LOSSLESS_STORES: [(&str, &str); 11] = [
    ("bandcamp.com", "Bandcamp"),
    ("qobuz.com", "Qobuz"),
    ("beatport.com", "Beatport"),
    ("7digital.com", "7digital"),
    ("hdtracks.com", "HDtracks"),
    ("junodownload.com", "Juno Download"),
    ("bleep.com", "Bleep"),
    ("boomkat.com", "Boomkat"),
    ("highresaudio.com", "HIGHRESAUDIO"),
    ("prestomusic.com", "Presto Music"),
    ("nativedsd.com", "NativeDSD"),
];

/// What a link sells, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Album,
    Single,
    Track,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuyLink {
    pub store: String,
    pub kind: LinkKind,
    pub url: String,
}

/// The track to find, with the album it's on.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuyQuery {
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub mb_release_group_id: Option<String>,
}

impl BuyQuery {
    fn album_artist(&self) -> &str {
        self.album_artist.as_deref().filter(|a| !a.is_empty()).unwrap_or(&self.artist)
    }
}

pub struct Stores {
    http: reqwest::Client,
    qobuz_locale: OnceCell<String>,
}

impl Stores {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, qobuz_locale: OnceCell::new() }
    }

    /// One link per store that has the track, preferring the album over the
    /// single track. Bandcamp first, then Qobuz, then the rest.
    pub async fn find(&self, musicbrainz: &MusicBrainz, query: &BuyQuery) -> Vec<BuyLink> {
        let (bandcamp, qobuz, listed) =
            tokio::join!(self.bandcamp(query), self.qobuz(query), self.musicbrainz_links(musicbrainz, query));
        let found = [log_failure("Bandcamp", bandcamp), log_failure("Qobuz", qobuz)].into_iter().flatten();
        best_per_store(found.chain(listed).collect())
    }

    // --- Bandcamp ---

    async fn bandcamp(&self, query: &BuyQuery) -> AppResult<Option<BuyLink>> {
        let album_search = async {
            if query.album.is_empty() {
                return Ok(Vec::new());
            }
            self.bandcamp_search(&format!("{} {}", query.album_artist(), query.album), "a").await
        };
        let track_text = format!("{} {}", query.artist, query.title);
        let track_search = self.bandcamp_search(&track_text, "t");
        let (albums, tracks) = tokio::join!(album_search, track_search);
        let (albums, tracks) = (albums?, tracks?);

        // Anyone can upload to Bandcamp under any artist name, so leave out
        // releases that say they're not the real thing.
        let albums: Vec<&BandcampItem> = albums.iter().filter(|a| a.kind == "a" && !unofficial(&a.name)).collect();
        let matched_tracks: Vec<&BandcampItem> = tracks
            .iter()
            .filter(|t| t.kind == "t" && same_title(&t.name, &query.title) && same_artist(&t.band_name, &query.artist))
            .filter(|t| !t.album_name.as_deref().is_some_and(unofficial))
            .collect();
        let on_track_album = |a: &BandcampItem| matched_tracks.iter().any(|t| t.album_id == Some(a.id));
        let album = best_title_match(
            albums.iter().copied().filter(|a| same_artist(&a.band_name, query.album_artist())),
            &query.album,
            |a| &a.name,
        )
        .or_else(|| albums.iter().copied().find(|a| on_track_album(a)));
        if let Some(album) = album {
            return Ok(Some(bandcamp_link(LinkKind::Album, album)));
        }

        // The album may go by another name on Bandcamp ("Deluxe" editions…):
        // look it up by the name the matched track gives.
        let Some(track) = best_title_match(matched_tracks.iter().copied(), &query.title, |t| &t.name) else {
            return Ok(None);
        };
        if let (Some(album_name), Some(_)) = (track.album_name.as_deref().filter(|n| !n.is_empty()), track.album_id) {
            let albums = self.bandcamp_search(&format!("{} {album_name}", track.band_name), "a").await?;
            if let Some(album) = albums.iter().find(|a| a.kind == "a" && on_track_album(a)) {
                return Ok(Some(bandcamp_link(LinkKind::Album, album)));
            }
        }
        Ok(Some(bandcamp_link(LinkKind::Track, track)))
    }

    /// Bandcamp's search box: `filter` is "a" for albums, "t" for tracks.
    async fn bandcamp_search(&self, text: &str, filter: &str) -> AppResult<Vec<BandcampItem>> {
        let body = serde_json::json!({
            "search_text": text,
            "search_filter": filter,
            "full_page": false,
            "fan_id": null,
        });
        let res: BandcampSearch =
            self.http.post(BANDCAMP_SEARCH).json(&body).send().await?.error_for_status()?.json().await?;
        Ok(res.auto.results)
    }

    // --- Qobuz ---

    async fn qobuz(&self, query: &BuyQuery) -> AppResult<Option<BuyLink>> {
        if !query.album.is_empty() {
            let albums = self.qobuz_search(&format!("{} {}", query.album_artist(), query.album)).await?;
            let by_artist = albums.iter().filter(|a| same_artist(&a.artist, query.album_artist()));
            if let Some(album) = best_title_match(by_artist, &query.album, |a| &a.title) {
                return Ok(Some(BuyLink { store: "Qobuz".into(), kind: LinkKind::Album, url: album.url.clone() }));
            }
        }
        // Qobuz sells tracks from their release's page, so the closest thing
        // to a track link is a single named after it.
        let releases = self.qobuz_search(&format!("{} {}", query.artist, query.title)).await?;
        let by_artist = releases.iter().filter(|a| same_artist(&a.artist, &query.artist));
        Ok(best_title_match(by_artist, &query.title, |a| &a.title)
            .map(|single| BuyLink { store: "Qobuz".into(), kind: LinkKind::Single, url: single.url.clone() }))
    }

    /// Albums from Qobuz's search page, which has no public API.
    async fn qobuz_search(&self, text: &str) -> AppResult<Vec<QobuzAlbum>> {
        let locale = self.qobuz_locale().await;
        let url = format!("{QOBUZ}/{locale}/search/albums/{}", urlencoding::encode(text));
        let html = self.http.get(url).send().await?.error_for_status()?.text().await?;
        Ok(parse_qobuz_search(&html))
    }

    /// The store Qobuz sends this computer to, like "ca-en". Album pages of
    /// other countries' stores are often missing.
    async fn qobuz_locale(&self) -> &str {
        self.qobuz_locale
            .get_or_init(|| async {
                let detected = match self.http.get(QOBUZ).send().await {
                    Ok(res) => res.url().path_segments().and_then(|mut s| s.next()).filter(|s| is_locale(s)).map(str::to_string),
                    Err(e) => {
                        warn!("Couldn't detect the Qobuz store: {e}");
                        None
                    }
                };
                detected.unwrap_or_else(|| QOBUZ_DEFAULT_LOCALE.to_string())
            })
            .await
    }

    // --- MusicBrainz ---

    /// Lossless stores MusicBrainz lists for the release group, if their pages
    /// still exist.
    async fn musicbrainz_links(&self, musicbrainz: &MusicBrainz, query: &BuyQuery) -> Vec<BuyLink> {
        let Some(group) = query.mb_release_group_id.as_deref().filter(|g| !g.is_empty()) else {
            return Vec::new();
        };
        let PurchaseLinks { urls, single } = match musicbrainz.purchase_links(group).await {
            Ok(links) => links,
            Err(e) => {
                warn!("MusicBrainz purchase links failed for {group}: {e}");
                return Vec::new();
            }
        };
        let locale = self.qobuz_locale().await.to_string();
        let kind = if single { LinkKind::Single } else { LinkKind::Album };
        let candidates = urls.into_iter().filter_map(|url| {
            let store = store_for(&url)?;
            let url = if store == "Qobuz" { with_qobuz_locale(&url, &locale) } else { url };
            Some(BuyLink { store: store.to_string(), kind, url })
        });
        let mut links = Vec::new();
        for link in candidates {
            if self.exists(&link.url).await {
                links.push(link);
            }
        }
        links
    }

    /// Whether a store page is there. Stores that block scripts (Beatport
    /// answers 403) get the benefit of the doubt; a 404 or 410 means it's gone.
    async fn exists(&self, url: &str) -> bool {
        match self.http.head(url).send().await {
            Ok(res) => {
                let status = res.status();
                status.is_success() || (status.is_client_error() && !matches!(status.as_u16(), 404 | 410))
            }
            Err(e) => {
                warn!("Couldn't check store link {url}: {e}");
                false
            }
        }
    }
}

// --- Matching ---

/// Same words, ignoring case, punctuation and annotations like "(Remastered)".
fn same_title(found: &str, wanted: &str) -> bool {
    !normalize(found).is_empty() && string_distance(found, wanted) == 0.0
}

fn same_artist(found: &str, wanted: &str) -> bool {
    !normalize(found).is_empty() && artist_distance(found, wanted) == 0.0
}

/// Bootlegs and promo copies uploaded by someone other than the artist.
fn unofficial(title: &str) -> bool {
    normalize(title).split(' ').any(|w| matches!(w, "promo" | "bootleg" | "unofficial"))
}

/// The item whose title is closest to the wanted one: exactly the same,
/// then the same apart from punctuation, then the same words. So "Three +" is
/// only taken when there's no "Three".
fn best_title_match<'a, T>(
    items: impl Iterator<Item = &'a T>,
    wanted: &str,
    title: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let exact = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let closeness = |item: &&T| {
        let found = title(item);
        if exact(found) == exact(wanted) {
            0
        } else if normalize(found) == normalize(wanted) {
            1
        } else {
            2
        }
    };
    // `min_by_key` keeps the first of equals, so search ranking breaks ties.
    items.filter(|item| same_title(title(item), wanted)).min_by_key(closeness)
}

/// One link per store: the album over a single over a track, and searches
/// over MusicBrainz's links on a tie. Ordered as in [`LOSSLESS_STORES`].
fn best_per_store(links: Vec<BuyLink>) -> Vec<BuyLink> {
    let mut best: Vec<BuyLink> = Vec::new();
    for link in links {
        match best.iter_mut().find(|b| b.store == link.store) {
            Some(existing) if link.kind < existing.kind => *existing = link,
            Some(_) => {}
            None => best.push(link),
        }
    }
    let rank = |store: &str| LOSSLESS_STORES.iter().position(|(_, name)| *name == store).unwrap_or(usize::MAX);
    best.sort_by_key(|link| rank(&link.store));
    best
}

fn store_for(url: &str) -> Option<&'static str> {
    let host = reqwest::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    LOSSLESS_STORES
        .iter()
        .find(|(domain, _)| host == *domain || host.ends_with(&format!(".{domain}")))
        .map(|(_, name)| *name)
}

fn log_failure(store: &str, result: AppResult<Option<BuyLink>>) -> Option<BuyLink> {
    result.unwrap_or_else(|e| {
        warn!("{store} search failed: {e}");
        None
    })
}

// --- Bandcamp responses ---

#[derive(Deserialize)]
struct BandcampSearch {
    auto: BandcampResults,
}

#[derive(Deserialize)]
struct BandcampResults {
    #[serde(default)]
    results: Vec<BandcampItem>,
}

#[derive(Deserialize)]
struct BandcampItem {
    /// "a" album, "t" track, "b" artist or label.
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    band_name: String,
    /// Tracks only.
    album_id: Option<u64>,
    album_name: Option<String>,
    item_url_path: String,
}

fn bandcamp_link(kind: LinkKind, item: &BandcampItem) -> BuyLink {
    BuyLink { store: "Bandcamp".into(), kind, url: item.item_url_path.clone() }
}

// --- Qobuz pages ---

#[derive(Debug, PartialEq)]
struct QobuzAlbum {
    title: String,
    artist: String,
    url: String,
}

/// Albums on a search page: titles and links from its structured data,
/// artists from each card's "More details on <album> by <artist>." label.
fn parse_qobuz_search(html: &str) -> Vec<QobuzAlbum> {
    const LD_OPEN: &str = r#"<script type="application/ld+json">"#;
    const DETAILS: &str = "More details on ";

    let mut albums = Vec::new();
    for block in html.split(LD_OPEN).skip(1) {
        let Some((json, _)) = block.split_once("</script>") else { continue };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(json.trim()) else { continue };
        let Some(items) = data.pointer("/mainEntity/itemListElement").and_then(|v| v.as_array()) else { continue };
        for entry in items {
            let item = &entry["item"];
            let (Some(title), Some(url)) = (item["name"].as_str(), item["url"].as_str()) else { continue };
            let Some(path) = url.strip_prefix(QOBUZ) else { continue };
            let Some(card) = html.split_once(&format!(r#"href="{path}""#)).map(|(_, rest)| rest) else { continue };
            let Some(label) = card
                .split_once(r#"title=""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(label, _)| decode_entities(label))
            else {
                continue;
            };
            let Some(credit) = label.strip_prefix(DETAILS).map(|l| l.strip_suffix('.').unwrap_or(l)) else { continue };
            let artist = credit
                .strip_prefix(&format!("{title} by "))
                .or_else(|| credit.rsplit_once(" by ").map(|(_, artist)| artist))
                .unwrap_or_default();
            albums.push(QobuzAlbum { title: title.to_string(), artist: artist.to_string(), url: url.to_string() });
        }
    }
    albums
}

/// "ca-en", "fr-fr"…
fn is_locale(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() == 5 && bytes[2] == b'-' && segment.chars().filter(|c| *c != '-').all(|c| c.is_ascii_lowercase())
}

/// Points a Qobuz page at the local store: /gb-en/album/… → /ca-en/album/….
fn with_qobuz_locale(url: &str, locale: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else { return url.to_string() };
    let segments: Vec<String> = parsed.path_segments().map(|s| s.map(str::to_string).collect()).unwrap_or_default();
    match segments.split_first() {
        Some((first, rest)) if is_locale(first) => {
            parsed.set_path(&format!("/{locale}/{}", rest.join("/")));
            parsed.to_string()
        }
        _ => url.to_string(),
    }
}

/// Decodes the HTML entities found in attribute values.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let decoded = match &rest[1..end] {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "nbsp" => Some(' '),
            entity => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const QOBUZ_PAGE: &str = r#"<head><script type="application/ld+json">
      {"@context":"https://schema.org","@type":"SearchResultsPage","mainEntity":{"@type":"ItemList","itemListElement":[
        {"@type":"ListItem","position":1,"url":"https://www.qobuz.com/ca-en/album/bookends-simon-garfunkel/0886445434617","item":{"@type":"MusicAlbum","name":"Bookends","url":"https://www.qobuz.com/ca-en/album/bookends-simon-garfunkel/0886445434617"}},
        {"@type":"ListItem","position":2,"url":"https://www.qobuz.com/ca-en/album/live-1969/x1","item":{"@type":"MusicAlbum","name":"Live 1969","url":"https://www.qobuz.com/ca-en/album/live-1969/x1"}}]}}
    </script></head><body>
      <a class="block" href="/ca-en/album/bookends-simon-garfunkel/0886445434617"
        title="More details on Bookends by Simon &amp; Garfunkel."></a>
      <a href="/ca-en/album/live-1969/x1" title="More details on Live 1969 by Simon &#x26; Garfunkel."></a>
    </body>"#;

    #[test]
    fn parses_qobuz_search_results() {
        let albums = parse_qobuz_search(QOBUZ_PAGE);
        assert_eq!(albums.len(), 2);
        assert_eq!(
            albums[0],
            QobuzAlbum {
                title: "Bookends".into(),
                artist: "Simon & Garfunkel".into(),
                url: "https://www.qobuz.com/ca-en/album/bookends-simon-garfunkel/0886445434617".into(),
            }
        );
        assert_eq!(albums[1].artist, "Simon & Garfunkel");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("Rock &amp; Roll &#039;69&#x27; &bogus; & more"), "Rock & Roll '69' &bogus; & more");
    }

    #[test]
    fn moves_qobuz_links_to_the_local_store() {
        assert_eq!(
            with_qobuz_locale("https://www.qobuz.com/gb-en/album/communicating-hundred-waters/0075679889294", "ca-en"),
            "https://www.qobuz.com/ca-en/album/communicating-hundred-waters/0075679889294"
        );
        assert_eq!(with_qobuz_locale("https://open.qobuz.com/album/123", "ca-en"), "https://open.qobuz.com/album/123");
    }

    #[test]
    fn recognizes_lossless_stores() {
        assert_eq!(store_for("https://hundredwaters.bandcamp.com/album/communicating-2"), Some("Bandcamp"));
        assert_eq!(store_for("https://www.beatport.com/release/three/4517476"), Some("Beatport"));
        assert_eq!(store_for("https://itunes.apple.com/us/album/id1268040402"), None);
        assert_eq!(store_for("https://notbandcamp.com/x"), None);
    }

    #[test]
    fn spots_unofficial_uploads() {
        assert!(unofficial("Since I Left You (Zomba Promo)"));
        assert!(!unofficial("Promotional Copy Blues"));
        assert!(!unofficial("Since I Left You"));
    }

    #[test]
    fn prefers_exact_titles() {
        let titles = ["Three +", "Three"];
        assert_eq!(best_title_match(titles.iter(), "Three", |t| t), Some(&"Three"));
        assert_eq!(best_title_match(titles.iter(), "Three Drums", |t| t), None);
    }

    #[test]
    fn keeps_the_best_link_per_store_in_store_order() {
        let link = |store: &str, kind| BuyLink { store: store.into(), kind, url: format!("{store}-{kind:?}") };
        let best = best_per_store(vec![
            link("Qobuz", LinkKind::Single),
            link("Beatport", LinkKind::Album),
            link("Bandcamp", LinkKind::Track),
            link("Bandcamp", LinkKind::Album),
            link("Qobuz", LinkKind::Album),
            link("Bandcamp", LinkKind::Album),
        ]);
        let got: Vec<_> = best.iter().map(|l| (l.store.as_str(), l.kind)).collect();
        assert_eq!(
            got,
            vec![("Bandcamp", LinkKind::Album), ("Qobuz", LinkKind::Album), ("Beatport", LinkKind::Album)]
        );
        assert_eq!(best[0].url, "Bandcamp-Album");
    }

    #[tokio::test]
    #[ignore = "live: hits Bandcamp, Qobuz and MusicBrainz"]
    async fn live_finds_store_links() {
        let http = reqwest::Client::builder().user_agent(super::super::USER_AGENT).build().unwrap();
        let stores = Stores::new(http.clone());
        let musicbrainz = MusicBrainz::new(http);
        let query = BuyQuery {
            title: "Wave to Anchor".into(),
            artist: "Hundred Waters".into(),
            album: "Communicating".into(),
            album_artist: None,
            mb_release_group_id: Some("0522609c-deda-45f1-8a9a-4ef9e6520ab3".into()),
        };
        let links = stores.find(&musicbrainz, &query).await;
        println!("{links:#?}");
        assert_eq!(links[0].store, "Bandcamp");
        assert_eq!(links[0].kind, LinkKind::Album);
        assert!(links.iter().any(|l| l.store == "Qobuz" && l.kind == LinkKind::Album));
    }
}

