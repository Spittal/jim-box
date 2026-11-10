//! Picks which Soulseek files could be the requested track, and in what order
//! to try them. Pure functions so the heuristics are easy to test and tune.

use std::collections::HashSet;

use crate::slskd::types::{SearchFile, SearchResponse};
use crate::state::{AudioFormat, Candidate, Settings, TrackMetadata};
use crate::text::{normalize, primary_artist, split_version, strip_noise, tokens, version_words};

/// Files whose length differs from the recording by more than this are a different version.
pub const MAX_LENGTH_DIFF_SECS: u32 = 5;
pub const MAX_CANDIDATES: usize = 50;
/// Lossy files below this bitrate (kbps) are never offered; Opus is
/// transparent at lower rates.
const MIN_LOSSY_KBPS: u32 = 192;
const MIN_OPUS_KBPS: u32 = 128;
/// Files kept, with their reason, after failing a check.
pub const MAX_REJECTED: usize = 30;

/// Words that mark an alternate version. A file containing one is rejected
/// unless the requested title contains it too.
pub const VERSION_WORDS: [&str; 13] = [
    "remix", "live", "instrumental", "karaoke", "acapella", "cappella", "demo", "cover", "edit",
    "mix", "acoustic", "reprise", "rehearsal",
];

/// A Soulseek search to run for a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    /// What, besides the title, ties results to the right track.
    pub anchor: Anchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The query names the artist, so every result already matches it.
    Artist,
    /// Searched by album instead of artist; results must be filed under the album.
    Album,
    /// Searched without the full artist name; results' paths must name the artist.
    ArtistInPath,
}

/// The searches to try for a track, most specific first, stopping at the first
/// with usable results. Soulseek only returns files matching every word, so
/// featured artists and "(feat. …)" are left out.
///
/// 1. Artist, title and version words ("madvillain money folder four tet remix"),
///    for remixes and other named versions.
/// 2. Artist and title, in case files spell the version differently.
/// 3. Album and title. The Soulseek server gives clients a list of banned
///    phrases (many major-label artist names), and compliant peers ignore
///    searches containing one, so "kendrick lamar alright" finds nothing
///    while "to pimp a butterfly alright" does. Skipped when the album is
///    named after the song; see `artist_free_queries` for those.
pub fn search_queries(meta: &TrackMetadata) -> Vec<SearchQuery> {
    let (name, _) = split_version(&meta.title);
    let artist = || tokens(primary_artist(&meta.artist)).into_iter();
    let mut texts = vec![
        (join_unique(artist().chain(tokens(&name)).chain(version_words(&meta.title))), Anchor::Artist),
        (join_unique(artist().chain(tokens(&name))), Anchor::Artist),
    ];
    let album = tokens(&strip_noise(&meta.album));
    // A single named after its song gives nothing to anchor results on.
    if !album.is_empty() && album != tokens(&name) {
        texts.push((join_unique(album.into_iter().chain(tokens(&name))), Anchor::Album));
    }
    dedup_queries(texts)
}

/// Searches that avoid a banned artist name, for when no peer answered any
/// search naming the artist. Banned phrases are whole names ("ariana grande"),
/// and the peers' files still have the name in their paths, so:
///
/// 1. The artist without their first word, the title and version words
///    ("grande thank u next").
/// 2. The title and version words alone.
///
/// Results must have the artist's full name somewhere in their path.
pub fn artist_free_queries(meta: &TrackMetadata) -> Vec<SearchQuery> {
    let artist = tokens(primary_artist(&meta.artist));
    if artist.is_empty() {
        return Vec::new();
    }
    let (name, _) = split_version(&meta.title);
    let title = || tokens(&name).into_iter().chain(version_words(&meta.title));
    let mut texts = Vec::new();
    if artist.len() > 1 {
        texts.push((join_unique(artist[1..].iter().cloned().chain(title())), Anchor::ArtistInPath));
    }
    texts.push((join_unique(title()), Anchor::ArtistInPath));
    dedup_queries(texts)
}

fn dedup_queries(texts: Vec<(String, Anchor)>) -> Vec<SearchQuery> {
    let mut queries: Vec<SearchQuery> = Vec::new();
    for (text, anchor) in texts {
        if !text.is_empty() && !queries.iter().any(|q| q.text == text) {
            queries.push(SearchQuery { text, anchor });
        }
    }
    queries
}

/// The files a search found with the requested title: lossless ones that
/// passed every check, best first, lossless ones that didn't, each with the
/// reason, and lossy ones that passed, as a fallback.
#[derive(Debug, Default)]
pub struct Ranked {
    pub accepted: Vec<Candidate>,
    pub rejected: Vec<Candidate>,
    pub lossy: Vec<Candidate>,
}

/// Ranks the results of `query`. Searches that don't name the artist can't
/// rely on it, so their files must be filed under the right album, or have
/// the artist in their path, to be accepted.
pub fn rank_for(query: &SearchQuery, responses: &[SearchResponse], meta: &TrackMetadata, settings: &Settings) -> Ranked {
    let ranked = rank(responses, meta, settings);
    match query.anchor {
        Anchor::Artist => ranked,
        Anchor::Album => {
            let album = normalize(&strip_noise(&meta.album));
            require_in_path(
                ranked,
                |c| normalize(split_path(&c.filename).0).contains(&album),
                format!("Not in a “{}” folder", strip_noise(&meta.album)),
            )
        }
        Anchor::ArtistInPath => {
            let artist = primary_artist(&meta.artist);
            let compact = normalize(artist).replace(' ', "");
            require_in_path(
                ranked,
                |c| normalize(&c.filename).replace(' ', "").contains(&compact),
                format!("Path doesn't mention “{artist}”"),
            )
        }
    }
}

/// Moves accepted files failing `keep` to the rejected ones, with `reason`,
/// and drops lossy ones failing it.
fn require_in_path(mut ranked: Ranked, keep: impl Fn(&Candidate) -> bool, reason: String) -> Ranked {
    let (accepted, mut outside): (Vec<_>, Vec<_>) = ranked.accepted.into_iter().partition(|c| keep(c));
    for c in &mut outside {
        c.rejected = Some(reason.clone());
    }
    ranked.accepted = accepted;
    ranked.rejected.extend(outside);
    ranked.lossy.retain(|c| keep(c));
    sort_and_cap(&mut ranked.rejected, MAX_REJECTED);
    ranked
}

fn join_unique(words: impl Iterator<Item = String>) -> String {
    let mut seen = HashSet::new();
    words.filter(|w| seen.insert(w.clone())).collect::<Vec<_>>().join(" ")
}

/// Filters and orders search results, best first. Lossless files with the
/// requested title that fail a check are kept as `rejected`, with the reason,
/// so they can be picked by hand.
pub fn rank(responses: &[SearchResponse], meta: &TrackMetadata, settings: &Settings) -> Ranked {
    let (name, _) = split_version(&meta.title);
    let title_tokens = tokens(&name);
    // Named versions ("Four Tet Remix") must appear somewhere in the path;
    // files often say "Re-Rub" for "ReRub", so compare without spaces.
    let version = version_words(&meta.title);
    let requested_words: HashSet<String> = tokens(&meta.title).into_iter().collect();
    let artist = normalize(&meta.artist);
    let album = normalize(&meta.album);
    let target_secs = meta.duration_ms.map(|ms| (ms + 500) / 1000);

    let mut ranked = Ranked::default();
    let files = responses
        .iter()
        .flat_map(|response| response.files.iter().map(move |file| (response, file)));
    for (response, file) in files {
        let Some(format) = detect_format(file) else { continue };
        let lossless = format.is_lossless();
        if lossless && !settings.formats.contains(&format) {
            continue;
        }
        let min_kbps = if format == AudioFormat::Opus { MIN_OPUS_KBPS } else { MIN_LOSSY_KBPS };
        if !lossless && file.bit_rate.is_some_and(|kbps| kbps < min_kbps) {
            continue;
        }

        let (dirs, basename) = split_path(&file.filename);
        let name_tokens: HashSet<String> = tokens(basename).into_iter().collect();
        if !title_tokens.iter().all(|t| name_tokens.contains(t)) {
            continue;
        }

        let mut rejected = None;
        let compact_path = normalize(&file.filename).replace(' ', "");
        if !version.iter().all(|w| compact_path.contains(w.as_str())) {
            rejected = Some(format!("Not the “{}” version", version.join(" ")));
        }

        let parent = dirs.rsplit(['\\', '/']).next().unwrap_or_default();
        let context: HashSet<String> = version_context(basename).into_iter().chain(version_context(parent)).collect();
        if let Some(word) = VERSION_WORDS
            .iter()
            .find(|w| context.contains(**w) && !requested_words.contains(**w))
        {
            rejected.get_or_insert_with(|| format!("Looks like a different version (“{word}”)"));
        }

        let mut score = format.preference();
        match (target_secs, file.length) {
            (Some(target), Some(len)) if len.abs_diff(target) > MAX_LENGTH_DIFF_SECS => {
                rejected.get_or_insert_with(|| format!("Length {}, expected {}", mmss(len), mmss(target)));
            }
            (Some(_), Some(_)) => score += 15,
            _ => score -= 10,
        }

        let path = normalize(&file.filename);
        if !artist.is_empty() && path.contains(&artist) {
            score += 25;
        }
        if !album.is_empty() && path.contains(&album) {
            score += 15;
        }

        if !lossless {
            score += match file.bit_rate {
                Some(kbps) => (kbps.min(320) / 8) as i64,
                None => -20,
            };
        }

        let hi_res = file.bit_depth.is_some_and(|b| b > 16)
            || file.sample_rate.is_some_and(|r| r > 48_000);
        if settings.prefer_hi_res && hi_res {
            score += 30;
        }

        if response.has_free_upload_slot {
            score += 40;
        }
        score -= response.queue_length.min(50) as i64;
        score += (response.upload_speed / 100_000).min(30) as i64;

        let candidate = Candidate {
            username: response.username.clone(),
            filename: file.filename.clone(),
            size: file.size,
            format,
            bit_depth: file.bit_depth,
            sample_rate: file.sample_rate,
            bit_rate: file.bit_rate,
            length_secs: file.length,
            has_free_upload_slot: response.has_free_upload_slot,
            queue_length: response.queue_length,
            upload_speed: response.upload_speed,
            score,
            failed: false,
            rejected,
        };
        if !lossless {
            // Lossy files failing a check aren't worth offering.
            if candidate.rejected.is_none() {
                ranked.lossy.push(candidate);
            }
        } else if candidate.rejected.is_some() {
            ranked.rejected.push(candidate);
        } else {
            ranked.accepted.push(candidate);
        }
    }

    sort_and_cap(&mut ranked.accepted, MAX_CANDIDATES);
    sort_and_cap(&mut ranked.rejected, MAX_REJECTED);
    sort_and_cap(&mut ranked.lossy, MAX_CANDIDATES);
    ranked
}

/// Best first, keeping at most `max`.
pub fn sort_and_cap(candidates: &mut Vec<Candidate>, max: usize) {
    candidates.sort_by(|a, b| b.score.cmp(&a.score));
    candidates.truncate(max);
}

/// Words that could mark an alternate version. "Original Mix", common in
/// scene and store releases, names the default cut, so its "mix" doesn't count.
pub fn version_context(s: &str) -> Vec<String> {
    let padded = format!(" {} ", normalize(s));
    tokens(&padded.replace(" original mix ", " "))
}

fn mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Splits a Soulseek path (usually Windows-style) into its directory and file name.
pub fn split_path(filename: &str) -> (&str, &str) {
    match filename.rfind(['\\', '/']) {
        Some(i) => (&filename[..i], &filename[i + 1..]),
        None => ("", filename),
    }
}

fn detect_format(file: &SearchFile) -> Option<AudioFormat> {
    let ext = file.filename.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "flac" => Some(AudioFormat::Flac),
        "aif" | "aiff" | "aifc" => Some(AudioFormat::Aiff),
        "wav" => Some(AudioFormat::Wav),
        // .m4a is usually lossy AAC; only lossless ALAC reports a bit depth or a
        // bitrate far above AAC's range.
        "m4a" | "alac"
            if file.bit_depth.is_some() || file.bit_rate.is_some_and(|b| b >= 500) =>
        {
            Some(AudioFormat::Alac)
        }
        "m4a" => Some(AudioFormat::Aac),
        "mp3" => Some(AudioFormat::Mp3),
        "ogg" => Some(AudioFormat::Vorbis),
        "opus" => Some(AudioFormat::Opus),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn meta() -> TrackMetadata {
        TrackMetadata {
            title: "One More Time".into(),
            artist: "Daft Punk".into(),
            album: "Discovery".into(),
            album_artist: "Daft Punk".into(),
            duration_ms: Some(320_357),
            ..Default::default()
        }
    }

    fn settings() -> Settings {
        Settings::new(PathBuf::from("/music"))
    }

    fn file(filename: &str, length: u32, bit_depth: Option<u32>) -> SearchFile {
        SearchFile {
            filename: filename.into(),
            size: 40_000_000,
            bit_depth,
            bit_rate: None,
            sample_rate: None,
            length: Some(length),
        }
    }

    fn peer(username: &str, free_slot: bool, files: Vec<SearchFile>) -> SearchResponse {
        SearchResponse {
            username: username.into(),
            files,
            has_free_upload_slot: free_slot,
            queue_length: 0,
            upload_speed: 1_000_000,
        }
    }

    fn filenames(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.filename.as_str()).collect()
    }

    fn texts(queries: &[SearchQuery]) -> Vec<&str> {
        queries.iter().map(|q| q.text.as_str()).collect()
    }

    #[test]
    fn builds_search_queries() {
        let mut m = meta();
        m.title = "Get Lucky (feat. Pharrell Williams)".into();
        assert_eq!(texts(&search_queries(&m)), vec!["daft punk get lucky", "discovery get lucky"]);
        m.artist = "Justin Bieber feat. Nicki Minaj".into();
        m.title = "Beauty and a Beat".into();
        assert_eq!(search_queries(&m)[0].text, "justin bieber beauty a beat");
    }

    #[test]
    fn searches_named_versions_first() {
        let mut m = meta();
        m.artist = "Madvillain".into();
        m.title = "Money Folder (Four Tet remix)".into();
        m.album = "Four Tet Remixes".into();
        assert_eq!(
            texts(&search_queries(&m)),
            vec!["madvillain money folder four tet remix", "madvillain money folder", "four tet remixes money folder"]
        );
        m.title = "Bayside (radio edit)".into();
        m.album = "Bayside".into();
        assert_eq!(texts(&search_queries(&m)), vec!["madvillain bayside"]);
    }

    #[test]
    fn falls_back_to_album_search() {
        let mut m = meta();
        m.title = "Alright".into();
        m.album = "To Pimp a Butterfly".into();
        let queries = search_queries(&m);
        assert_eq!(queries.last(), Some(&SearchQuery { text: "to pimp a butterfly alright".into(), anchor: Anchor::Album }));
        m.album = "Alright".into();
        assert!(search_queries(&m).iter().all(|q| q.anchor == Anchor::Artist));
    }

    #[test]
    fn album_search_requires_album_in_path() {
        let mut m = meta();
        m.title = "Alright".into();
        m.artist = "Kendrick Lamar".into();
        m.album = "To Pimp a Butterfly".into();
        m.duration_ms = Some(219_333);
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"Music\K. Lamar\To Pimp A Butterfly (2015)\07 - Alright.flac", 219, Some(16)),
                file(r"Music\Someone\Other Album\03 Alright.flac", 219, Some(16)),
            ],
        )];
        let query = search_queries(&m).pop().unwrap();
        let ranked = rank_for(&query, &responses, &m, &settings());
        assert_eq!(filenames(&ranked.rejected), vec![r"Music\Someone\Other Album\03 Alright.flac"]);
        assert_eq!(ranked.rejected[0].rejected.as_deref(), Some("Not in a “To Pimp a Butterfly” folder"));
        assert_eq!(filenames(&ranked.accepted), vec![r"Music\K. Lamar\To Pimp A Butterfly (2015)\07 - Alright.flac"]);
    }

    #[test]
    fn searches_around_banned_artist_names() {
        let mut m = meta();
        m.artist = "Ariana Grande".into();
        m.title = "thank u, next".into();
        m.album = "thank u, next".into();
        assert_eq!(texts(&search_queries(&m)), vec!["ariana grande thank u next"]);
        let queries = artist_free_queries(&m);
        assert_eq!(texts(&queries), vec!["grande thank u next", "thank u next"]);
        assert!(queries.iter().all(|q| q.anchor == Anchor::ArtistInPath));

        m.artist = "Drake feat. Rihanna".into();
        m.title = "Take Care (Four Tet Remix)".into();
        assert_eq!(texts(&artist_free_queries(&m)), vec!["take care four tet remix"]);
        m.artist = String::new();
        assert!(artist_free_queries(&m).is_empty());
    }

    #[test]
    fn artist_free_search_requires_artist_in_path() {
        let mut m = meta();
        m.artist = "Ariana Grande".into();
        m.title = "thank u, next".into();
        m.album = "thank u, next".into();
        m.duration_ms = Some(207_000);
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"@@zsttx\Music\Ariana Grande\Albums\2019 - thank u, next\CD1\11 - thank u, next.flac", 207, Some(16)),
                file(r"Music\ArianaGrande-Singles\thank u next.flac", 207, Some(16)),
                file(r"Music\Covers\Someone - thank u, next.flac", 207, Some(16)),
            ],
        )];
        let query = artist_free_queries(&m).pop().unwrap();
        let ranked = rank_for(&query, &responses, &m, &settings());
        assert_eq!(ranked.accepted.len(), 2);
        assert_eq!(filenames(&ranked.rejected), vec![r"Music\Covers\Someone - thank u, next.flac"]);
        assert_eq!(ranked.rejected[0].rejected.as_deref(), Some("Path doesn't mention “Ariana Grande”"));
    }

    #[test]
    fn requires_named_versions_but_not_radio_edits() {
        let mut m = meta();
        m.title = "Rose rouge (Nightmares on Wax rerub)".into();
        m.duration_ms = Some(435_000);
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"Music\St Germain\Tourist\01 Rose Rouge.flac", 435, Some(16)),
                file(r"Music\St Germain\Singles\Rose Rouge (Nightmares On Wax Re-Rub).flac", 435, Some(16)),
            ],
        )];
        assert_eq!(
            filenames(&rank(&responses, &m, &settings()).accepted),
            vec![r"Music\St Germain\Singles\Rose Rouge (Nightmares On Wax Re-Rub).flac"]
        );

        m.title = "Bayside (radio edit)".into();
        m.duration_ms = Some(197_000);
        let responses = vec![peer("a", true, vec![file(r"Music\Obskur\Sport School\02 Bayside.flac", 197, Some(16))])];
        assert_eq!(rank(&responses, &m, &settings()).accepted.len(), 1);
    }

    #[test]
    fn rejects_lossy_wrong_versions_and_wrong_lengths() {
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"Music\Daft Punk\Discovery\01 One More Time.flac", 320, Some(16)),
                file(r"Music\Daft Punk\Discovery\01 One More Time.mp3", 320, None),
                file(r"Music\Daft Punk\Singles\One More Time (Radio Edit).flac", 320, Some(16)),
                file(r"Music\Daft Punk\Alive 2007\One More Time.flac", 400, Some(16)),
                file(r"Music\Daft Punk\Discovery\02 Aerodynamic.flac", 320, Some(16)),
                file(r"Music\Daft Punk\Discovery\One More Time.m4a", 320, None),
            ],
        )];
        let ranked = rank(&responses, &meta(), &settings()).accepted;
        assert_eq!(filenames(&ranked), vec![r"Music\Daft Punk\Discovery\01 One More Time.flac"]);
    }

    #[test]
    fn prefers_hi_res_and_free_slots() {
        let responses = vec![
            peer("busy", false, vec![file(r"DP\Discovery\01 One More Time.flac", 320, Some(24))]),
            peer("cd", true, vec![file(r"DP\Discovery\01 One More Time.flac", 320, Some(16))]),
            peer("hires", true, vec![file(r"DP\Discovery\01 One More Time.flac", 320, Some(24))]),
        ];
        let ranked = rank(&responses, &meta(), &settings()).accepted;
        let users: Vec<&str> = ranked.iter().map(|c| c.username.as_str()).collect();
        assert_eq!(users, vec!["hires", "cd", "busy"]);
    }

    #[test]
    fn respects_allowed_formats() {
        let mut s = settings();
        s.formats = vec![AudioFormat::Flac];
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"DP\One More Time.wav", 320, Some(16)),
                file(r"DP\One More Time.m4a", 320, Some(16)),
            ],
        )];
        assert!(rank(&responses, &meta(), &s).accepted.is_empty());
    }

    #[test]
    fn allows_version_words_the_user_asked_for() {
        let mut m = meta();
        m.title = "One More Time (Radio Edit)".into();
        m.duration_ms = Some(235_000);
        let responses = vec![peer(
            "a",
            true,
            vec![file(r"DP\Singles\One More Time (Radio Edit).flac", 235, Some(16))],
        )];
        assert_eq!(rank(&responses, &m, &settings()).accepted.len(), 1);
    }

    #[test]
    fn original_mix_is_the_default_cut() {
        let mut m = meta();
        m.artist = "Hundred Waters".into();
        m.title = "Wave to Anchor".into();
        m.album = "Communicating".into();
        m.duration_ms = Some(279_000);
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"Hundred_Waters-Communicating-WEB-FLAC-2017-DASH\02-hundred_waters-wave_to_anchor_(original_mix).flac", 279, Some(16)),
                file(r"Hundred Waters\Remixes\Wave to Anchor (Club Mix).flac", 279, Some(16)),
            ],
        )];
        let ranked = rank(&responses, &m, &settings());
        assert_eq!(
            filenames(&ranked.accepted),
            vec![r"Hundred_Waters-Communicating-WEB-FLAC-2017-DASH\02-hundred_waters-wave_to_anchor_(original_mix).flac"]
        );
        assert_eq!(filenames(&ranked.rejected), vec![r"Hundred Waters\Remixes\Wave to Anchor (Club Mix).flac"]);
    }

    #[test]
    fn keeps_rejected_files_with_reasons() {
        let mut m = meta();
        m.title = "Money Folder (Four Tet Remix)".into();
        let responses = vec![peer(
            "a",
            true,
            vec![
                file(r"Music\Madvillain\Madvillainy\05 Money Folder.flac", 320, Some(16)),
                file(r"Music\Madvillain\Remixes\Money Folder (Four Tet Remix).flac", 400, Some(16)),
                file(r"Music\Madvillain\Madvillainy\05 Money Folder.mp3", 320, None),
                file(r"Music\Madvillain\Madvillainy\06 Shadows of Tomorrow.flac", 320, Some(16)),
            ],
        )];
        let ranked = rank(&responses, &m, &settings());
        assert!(ranked.accepted.is_empty());
        let reasons: Vec<_> = ranked.rejected.iter().map(|c| c.rejected.as_deref().unwrap()).collect();
        assert_eq!(reasons.len(), 2, "lossy files and other titles are dropped entirely");
        assert!(reasons.contains(&"Not the “four tet remix” version"));
        assert!(reasons.contains(&"Length 6:40, expected 5:20"));
    }

    #[test]
    fn offers_lossy_files_as_a_fallback() {
        let lossy = |name: &str, length: u32, kbps: Option<u32>| {
            let mut f = file(name, length, None);
            f.bit_rate = kbps;
            f
        };
        let responses = vec![peer(
            "a",
            true,
            vec![
                lossy(r"DP\Discovery\01 One More Time.mp3", 320, Some(320)),
                lossy(r"DP\Discovery\01 One More Time v0.mp3", 320, Some(245)),
                lossy(r"DP\Discovery\01 One More Time 128.mp3", 320, Some(128)),
                lossy(r"DP\Alive\One More Time.mp3", 400, Some(320)),
                lossy(r"DP\Discovery\01 One More Time.m4a", 320, Some(256)),
                lossy(r"DP\Discovery\01 One More Time.opus", 320, Some(160)),
            ],
        )];
        let ranked = rank(&responses, &meta(), &settings());
        assert!(ranked.accepted.is_empty() && ranked.rejected.is_empty());
        assert_eq!(
            filenames(&ranked.lossy),
            vec![
                r"DP\Discovery\01 One More Time.mp3",
                r"DP\Discovery\01 One More Time v0.mp3",
                r"DP\Discovery\01 One More Time.m4a",
                r"DP\Discovery\01 One More Time.opus",
            ]
        );
        assert_eq!(ranked.lossy[2].format, AudioFormat::Aac);
    }

    #[test]
    fn splits_windows_paths() {
        assert_eq!(split_path(r"@@abc\Music\x.flac"), (r"@@abc\Music", "x.flac"));
        assert_eq!(split_path("x.flac"), ("", "x.flac"));
    }

    /// Runs the real searches and ranking against a running slskd. Start one
    /// (e.g. the app's) and pass its API port and key:
    /// `SLSKD_PORT=… SLSKD_API_KEY=… cargo test live_soulseek -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_soulseek_search() {
        let (Ok(port), Ok(key)) = (std::env::var("SLSKD_PORT"), std::env::var("SLSKD_API_KEY")) else {
            println!("Skipped: set SLSKD_PORT and SLSKD_API_KEY to run against slskd");
            return;
        };
        let client = crate::slskd::SlskdClient::new(port.parse().unwrap(), key);
        let track = |title: &str, artist: &str, album: &str, secs: u32| TrackMetadata {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            duration_ms: Some(secs * 1000),
            ..Default::default()
        };
        let tracks = [
            track("Wave to Anchor", "Hundred Waters", "Communicating", 279),
            track("sslip", "K-Lone", "sorry i thought you were someone else", 245),
            track("Mechta", "Lowres", "Mechta EP", 315),
            track("Sugar", "Hitchhiker", "Sugar - SM STATION", 186),
            // "ariana grande" is a banned phrase.
            track("thank u, next", "Ariana Grande", "thank u, next", 207),
        ];
        let mut wave_to_anchor = None;
        let mut mechta_lossy = 0;
        let mut thank_u_next = 0;
        for meta in &tracks {
            let mut queries = search_queries(meta);
            let mut next = 0;
            let mut artist_answered = false;
            while let Some(query) = queries.get(next).cloned() {
                next += 1;
                let responses = client.search(&query.text).await.unwrap();
                let ranked = rank_for(&query, &responses, meta, &settings());
                println!(
                    "\n“{}”: {} peers, {} accepted, {} rejected, {} lossy",
                    query.text,
                    responses.len(),
                    ranked.accepted.len(),
                    ranked.rejected.len(),
                    ranked.lossy.len()
                );
                for c in ranked.accepted.iter().take(3) {
                    println!("  ok   {:>4} {}", c.score, c.filename);
                }
                for c in ranked.rejected.iter().take(5) {
                    println!("  no   {:>4} {} [{}]", c.score, c.filename, c.rejected.as_deref().unwrap_or(""));
                }
                for c in ranked.lossy.iter().take(3) {
                    println!("  lossy {:>4} {:?} {:?}k {}", c.score, c.format, c.bit_rate, c.filename);
                }
                artist_answered |= query.anchor == Anchor::Artist && !responses.is_empty();
                if meta.title == "thank u, next" {
                    thank_u_next += ranked.accepted.len();
                }
                if meta.title == "Mechta" {
                    mechta_lossy += ranked.lossy.len();
                }
                if meta.title == "Wave to Anchor" && wave_to_anchor.is_none() {
                    wave_to_anchor = Some(ranked.accepted);
                }
                if responses.iter().any(|r| !r.files.is_empty()) {
                    break;
                }
                if next == queries.len() && !artist_answered && queries.iter().all(|q| q.anchor != Anchor::ArtistInPath) {
                    queries.extend(artist_free_queries(meta));
                }
            }
        }
        assert!(thank_u_next > 0, "thank u, next should be found without the banned artist name");
        assert!(mechta_lossy > 0, "Mechta is only shared lossy; it should be offered as a fallback");
        let accepted = wave_to_anchor.unwrap();
        assert!(!accepted.is_empty(), "Wave to Anchor should be found");
        println!(
            "\nWave to Anchor original-mix copies accepted: {}",
            accepted.iter().filter(|c| c.filename.contains("original_mix")).count()
        );
    }
}
