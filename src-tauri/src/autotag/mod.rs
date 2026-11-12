//! Identifying music, after beets' `autotag`: turning what we know about a
//! track or a folder (search results, tags, file names, playlist entries) into
//! MusicBrainz metadata, with a confidence the UI uses to decide what needs
//! the user's review.
//!
//! Deezer and MusicBrainz are both searched; Deezer's ranking is better for
//! popular music, MusicBrainz covers what Deezer lacks. The pure matching
//! logic lives in the submodules; this module wires it to the sources.

mod album;
mod distance;
mod hint;
mod release;
mod track;

pub use album::{AlbumMatch, FileTrack};
pub use distance::{artist_distance, string_distance};
pub use hint::{hint_from_file, TrackHint};

use std::path::Path;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::sources::musicbrainz::AlbumRelease;
use crate::sources::Sources;
use crate::state::{Confidence, ExistingTags, TrackMetadata, TrackRequest};
use crate::text::{lead_artist, split_version, tokens};

/// Extra MusicBrainz versions shown after Deezer's results.
const MAX_EXTRA_RESULTS: usize = 5;

/// Metadata to tag a picked search result with: MusicBrainz when it knows the
/// recording, otherwise Deezer's full track details. Never fails: the search
/// result itself is the last resort.
pub async fn resolve(sources: &Sources, request: &TrackRequest) -> TrackMetadata {
    let hint = TrackHint::from(request);
    match lookup(sources, &hint, request.mb_recording_id.as_deref()).await {
        Ok(Some(mut meta)) => {
            // Without a MusicBrainz release, the album comes from Deezer; so
            // must its track number, which search results leave out.
            if meta.mb_release_id.is_none() && meta.track_number.is_none() {
                if let Some(detailed) = deezer_details(sources, request).await {
                    meta.track_number = detailed.track_number;
                    meta.disc_number = meta.disc_number.or(detailed.disc_number);
                    meta.date = meta.date.or(detailed.release_date);
                }
            }
            return meta;
        }
        Ok(None) => {}
        Err(e) => warn!("MusicBrainz lookup failed for {}: {e}", request.title),
    }
    let detailed = deezer_details(sources, request).await;
    TrackMetadata::from_request(detailed.as_ref().unwrap_or(request))
}

/// The search result that is the same recording as `hint`, from Deezer or,
/// for tracks Deezer lacks, MusicBrainz.
pub async fn find_track(sources: &Sources, hint: &TrackHint) -> AppResult<Option<(TrackRequest, Confidence)>> {
    let results = sources.deezer.search(&hint.query()).await?;
    if let Some((index, confidence)) = track::best_match(&results, hint) {
        return Ok(Some((results[index].clone(), confidence)));
    }
    let recordings = sources.musicbrainz.search_recordings(&hint.title, lead_artist(&hint.artist)).await?;
    let results: Vec<TrackRequest> = recordings.iter().map(release::request_for).collect();
    Ok(track::best_match(&results, hint).map(|(index, confidence)| (results[index].clone(), confidence)))
}

/// Search results for typed text: Deezer's, then MusicBrainz versions Deezer
/// doesn't have. The user picks between them.
pub async fn search(sources: &Sources, query: &str) -> AppResult<Vec<TrackRequest>> {
    let (deezer, musicbrainz) = tokio::join!(sources.deezer.search(query), sources.musicbrainz.search_text(query));
    let mut results = deezer?;
    let recordings = musicbrainz.unwrap_or_else(|e| {
        warn!("MusicBrainz search failed: {e}");
        Vec::new()
    });

    let query_words = tokens(query);
    let mut extras: Vec<TrackRequest> = Vec::new();
    for candidate in recordings.iter().map(release::request_for) {
        // MusicBrainz's text search also returns covers and tributes; keep
        // results whose artist and title were both typed.
        let typed = |text: &str| tokens(text).iter().all(|w| query_words.contains(w));
        let relevant = typed(lead_artist(&candidate.artist)) && typed(&split_version(&candidate.title).0);
        let seen = results.iter().chain(&extras).any(|r| same_track(r, &candidate));
        if relevant && !seen && extras.len() < MAX_EXTRA_RESULTS {
            extras.push(candidate);
        }
    }
    results.extend(extras);
    Ok(results)
}

/// A search result for known metadata, to download a lossless copy of a
/// library file. Only confident matches, since the download replaces the file.
pub async fn lossless_source(sources: &Sources, meta: &TrackMetadata) -> Option<TrackRequest> {
    match find_track(sources, &TrackHint::from(meta)).await {
        Ok(Some((request, Confidence::High))) => Some(request),
        _ => None,
    }
}

/// A single file identified on its own.
pub struct Identified {
    pub metadata: TrackMetadata,
    pub confidence: Confidence,
    /// The search result it was matched through, if any.
    pub request: Option<TrackRequest>,
}

/// Identifies one file: by its MusicBrainz or ISRC tags when present,
/// otherwise by searching with its tags or file name.
pub async fn identify_file(
    sources: &Sources,
    tags: &ExistingTags,
    relative_path: &Path,
    duration_secs: Option<u32>,
) -> AppResult<Option<Identified>> {
    let hint = hint_from_file(tags, relative_path, duration_secs).unwrap_or_default();
    if let Some(metadata) = lookup(sources, &hint, tags.mb_recording_id.as_deref()).await? {
        return Ok(Some(Identified { metadata, confidence: Confidence::High, request: None }));
    }
    if hint.title.is_empty() {
        return Ok(None);
    }
    let Some((request, confidence)) = find_track(sources, &hint).await? else {
        return Ok(None);
    };
    let metadata = resolve(sources, &request).await;
    Ok(Some(Identified { metadata, confidence, request: Some(request) }))
}

/// The genres MusicBrainz has for a file's recording, the likeliest first.
/// `recording_id` is the recording it's known to be, if it has been
/// identified; otherwise it's found by the file's tags or name. Empty when
/// the recording isn't found, or nobody has given it a genre.
pub async fn genres(
    sources: &Sources,
    tags: &ExistingTags,
    relative_path: &Path,
    duration_secs: Option<u32>,
    recording_id: Option<String>,
) -> AppResult<Vec<String>> {
    let hint = hint_from_file(tags, relative_path, duration_secs).unwrap_or_default();
    let known = recording_id.or_else(|| tags.mb_recording_id.clone());
    let mut id = recording_id_for(sources, &hint, known).await?;
    if id.is_none() && !hint.title.is_empty() {
        if let Some((request, _)) = find_track(sources, &hint).await? {
            id = recording_id_for(sources, &TrackHint::from(&request), request.mb_recording_id.clone()).await?;
        }
    }
    match id {
        Some(id) => sources.musicbrainz.genres(&id).await,
        None => Ok(Vec::new()),
    }
}

/// The MusicBrainz recording a hint is of: the one it names, else its ISRC's.
async fn recording_id_for(sources: &Sources, hint: &TrackHint, known: Option<String>) -> AppResult<Option<String>> {
    if known.is_some() {
        return Ok(known);
    }
    let Some(isrc) = &hint.isrc else { return Ok(None) };
    let candidates = sources.musicbrainz.isrc_recordings(isrc).await?;
    Ok(track::pick_recording(&candidates, hint).map(|r| r.id.clone()))
}

/// A folder matched to a release as a whole.
pub struct AlbumCandidate {
    pub release: AlbumRelease,
    pub found: AlbumMatch,
    pub confidence: Confidence,
}

/// Matches a folder of files to a MusicBrainz release, beets-style. `None`
/// when no release fits well enough; the caller then identifies files one by
/// one. `folder_name` stands in for the album when the files have no tags.
pub async fn match_album(
    sources: &Sources,
    files: &[FileTrack],
    folder_name: Option<&str>,
) -> AppResult<Option<AlbumCandidate>> {
    let mb = &sources.musicbrainz;

    // Candidates: releases the files are already tagged with, releases one of
    // their recordings is on, and a search by album and artist.
    let mut tagged: Vec<&str> = files.iter().filter_map(|f| f.mb_release_id.as_deref()).collect();
    tagged.sort_unstable();
    tagged.dedup();
    tagged.truncate(2);
    let mut summaries = Vec::new();
    if let Some(recording) = files.iter().find_map(|f| f.mb_recording_id.as_deref()) {
        if let Some(recording) = mb.recording(recording).await? {
            summaries.extend(recording.appearances.into_iter().map(|a| a.release));
        }
    }
    let album_name = distance::majority(files.iter().map(|f| f.album.as_str())).or(folder_name);
    let artist = distance::majority(files.iter().map(FileTrack::credited_artist)).unwrap_or_default();
    if let Some(album_name) = album_name {
        summaries.extend(mb.search_releases(album_name, artist).await?);
    }

    let mut ids: Vec<String> = tagged.iter().map(|id| id.to_string()).collect();
    for summary in album::rank_candidates(summaries, files) {
        if ids.len() >= tagged.len() + album::MAX_CANDIDATES {
            break;
        }
        if !ids.contains(&summary.id) {
            ids.push(summary.id);
        }
    }

    // Fetch each candidate's tracklist and keep the closest.
    let mut best: Option<(AlbumRelease, AlbumMatch)> = None;
    for id in ids {
        let release = match mb.release(&id).await {
            Ok(Some(release)) => release,
            Ok(None) => continue,
            Err(e) => {
                warn!("Couldn't fetch release {id}: {e}");
                continue;
            }
        };
        let found = album::score(files, &release);
        if best.as_ref().map_or(true, |(r, m)| album::better((&release, &found), (r, m))) {
            best = Some((release, found));
        }
    }

    let Some((release, found)) = best else { return Ok(None) };
    info!(
        "Album match: {} of {} files on '{}' (distance {:.3})",
        found.matched(),
        files.len(),
        release.summary.title,
        found.distance
    );
    Ok(album::confidence(&found, files.len()).map(|confidence| AlbumCandidate { release, found, confidence }))
}

/// MusicBrainz metadata by recording ID, else by ISRC.
async fn lookup(sources: &Sources, hint: &TrackHint, recording_id: Option<&str>) -> AppResult<Option<TrackMetadata>> {
    let mb = &sources.musicbrainz;
    if let Some(id) = recording_id {
        if let Some(recording) = mb.recording(id).await? {
            return Ok(Some(release::metadata_for(&recording, hint)));
        }
    }
    let Some(isrc) = &hint.isrc else { return Ok(None) };
    let candidates = mb.isrc_recordings(isrc).await?;
    let Some(best) = track::pick_recording(&candidates, hint) else { return Ok(None) };
    Ok(mb.recording(&best.id).await?.map(|recording| release::metadata_for(&recording, hint)))
}

async fn deezer_details(sources: &Sources, request: &TrackRequest) -> Option<TrackRequest> {
    let id = request.deezer_id?;
    match sources.deezer.track(id).await {
        Ok(detailed) => Some(detailed),
        Err(e) => {
            warn!("Deezer track lookup failed for {}: {e}", request.title);
            None
        }
    }
}

/// Two search results for the same recording: same ISRC, or same title and
/// artist within a few seconds.
fn same_track(a: &TrackRequest, b: &TrackRequest) -> bool {
    if a.isrc.is_some() && a.isrc == b.isrc {
        return true;
    }
    track::best_match(std::slice::from_ref(a), &TrackHint::from(b)).is_some_and(|(_, c)| c == Confidence::High)
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Hits Deezer and MusicBrainz. Run with `cargo test -- --ignored live_search --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_search() {
        let sources = Sources::new();
        for query in ["hundred waters wave to anchor", "daft punk one more time", "sophie immaterial"] {
            let results = search(&sources, query).await.unwrap();
            println!("== {query}");
            for r in &results {
                let source = if r.deezer_id.is_some() { "deezer" } else { "mb" };
                println!("  {source:6} {} - {} ({}, {}s)", r.artist, r.title, r.album, r.duration_secs);
            }
            assert!(!results.is_empty());
        }
    }

    /// Hits Deezer and MusicBrainz. Run with `cargo test -- --ignored live_genres --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_genres() {
        let sources = Sources::new();
        for path in ["Daft Punk - One More Time.mp3", "Hundred Waters - Wave to Anchor.mp3"] {
            let found = genres(&sources, &ExistingTags::default(), Path::new(path), None, None).await.unwrap();
            println!("{path}: {found:?}");
            assert!(!found.is_empty());
        }
    }
}
