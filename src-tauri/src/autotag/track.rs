//! Deciding which search result or recording is the same recording as a
//! track we already know about: a library file or a playlist entry.

use super::hint::TrackHint;
use crate::sources::musicbrainz::RecordingRef;
use crate::state::{Confidence, TrackRequest};
use crate::text::{normalize, strip_noise, tokens, version_words};

/// Durations within this many seconds count as the same recording.
const SAME_LENGTH_SECS: u32 = 3;
/// Beyond this, it's a different version (edit, live, remaster with extra silence…).
const MAX_LENGTH_DIFF_SECS: u32 = 10;

/// Picks the search result that is the same recording as the file, if any.
pub fn best_match(results: &[TrackRequest], hint: &TrackHint) -> Option<(usize, Confidence)> {
    let title = tokens(&strip_noise(&hint.title));
    let artist = normalize(&hint.artist);

    results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if tokens(&strip_noise(&r.title)) != title {
                return None;
            }
            let result_artist = normalize(&r.artist);
            let artist_matches = !artist.is_empty()
                && (result_artist == artist
                    || result_artist.contains(&artist)
                    || artist.contains(&result_artist));
            let diff = hint.duration_secs.map(|d| d.abs_diff(r.duration_secs));
            if diff.is_some_and(|d| d > MAX_LENGTH_DIFF_SECS) {
                return None;
            }
            let confidence = match (artist_matches, diff) {
                (true, Some(d)) if d <= SAME_LENGTH_SECS => Confidence::High,
                (true, _) => Confidence::Medium,
                // Right title and length but different artist credit: worth a look, not a default.
                (false, Some(d)) if d <= SAME_LENGTH_SECS && !artist.is_empty() => return None,
                (false, Some(d)) if d <= SAME_LENGTH_SECS => Confidence::Medium,
                _ => return None,
            };
            let closeness = diff.unwrap_or(MAX_LENGTH_DIFF_SECS);
            Some((i, confidence, closeness))
        })
        // Prefer high confidence, then closest length, then search rank.
        .min_by_key(|&(i, confidence, closeness)| (confidence != Confidence::High, closeness, i))
        .map(|(i, confidence, _)| (i, confidence))
}

/// An ISRC sometimes covers several recordings, e.g. a song and its 5.1 or
/// club mix. Prefer the one whose version matches the requested title, then
/// the closest in length.
pub fn pick_recording<'a>(recordings: &'a [RecordingRef], hint: &TrackHint) -> Option<&'a RecordingRef> {
    let wanted = version_words(&hint.title);
    let target_ms = hint.duration_secs.unwrap_or_default() * 1000;
    recordings.iter().min_by_key(|r| {
        let words = version_words(&r.title);
        let mismatch = words.iter().filter(|w| !wanted.contains(w)).count()
            + wanted.iter().filter(|w| !words.contains(w)).count();
        (mismatch, r.length_ms.map_or(u32::MAX, |l| l.abs_diff(target_ms)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(title: &str, artist: &str, secs: u32) -> TrackRequest {
        TrackRequest {
            deezer_id: Some(1),
            mb_recording_id: None,
            title: title.into(),
            artist: artist.into(),
            album: "Album".into(),
            isrc: None,
            duration_secs: secs,
            track_number: None,
            disc_number: None,
            release_date: None,
            cover_url: None,
            cover_xl_url: None,
            preview_url: None,
        }
    }

    fn hint(title: &str, artist: &str, secs: Option<u32>) -> TrackHint {
        TrackHint { title: title.into(), artist: artist.into(), duration_secs: secs, ..Default::default() }
    }

    #[test]
    fn picks_the_recording_matching_the_requested_version() {
        let rec = |id: &str, title: &str, length| RecordingRef { id: id.into(), title: title.into(), length_ms: Some(length) };
        let doobies = [rec("5.1", "Long Train Runnin’ (5.1 mix)", 207_000), rec("stereo", "Long Train Runnin’", 207_266)];
        assert_eq!(pick_recording(&doobies, &hint("Long Train Runnin'", "", Some(207))).unwrap().id, "stereo");
        let foxx = [rec("album", "Blame It", 290_000), rec("club", "Blame It (club mix)", 289_746)];
        assert_eq!(pick_recording(&foxx, &hint("Blame It (feat. T-Pain) (Club Mix)", "", Some(290))).unwrap().id, "club");
    }

    #[test]
    fn matches_on_title_artist_and_length() {
        let hint = hint("Alright", "Kendrick Lamar", Some(220));
        let results = vec![
            request("Alright (Remix)", "Kendrick Lamar", 220),
            request("Alright", "Someone Else", 220),
            request("Alright", "Kendrick Lamar", 219),
        ];
        assert_eq!(best_match(&results, &hint), Some((2, Confidence::High)));
    }

    #[test]
    fn rejects_different_lengths_and_downgrades_unknown_length() {
        let mut hint = hint("Alright", "Kendrick Lamar", Some(300));
        let results = vec![request("Alright", "Kendrick Lamar", 219)];
        assert_eq!(best_match(&results, &hint), None);
        hint.duration_secs = None;
        assert_eq!(best_match(&results, &hint), Some((0, Confidence::Medium)));
    }
}
