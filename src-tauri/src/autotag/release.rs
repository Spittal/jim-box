//! Choosing which release to tag a single recording with, like beets'
//! `match.preferred` settings: studio albums over singles, CD/digital over
//! vinyl, the earliest edition, and the album the user picked when it's there.

use super::distance::earlier_date;
use super::hint::TrackHint;
use crate::sources::musicbrainz::{self, Recording, ReleaseKind, ReleaseStatus, ReleaseSummary};
use crate::state::{TrackMetadata, TrackRequest};
use crate::text::normalize;

/// The recording's metadata on its best release (see [`pick_release`]), with
/// gaps filled from the hint. Without a suitable release, the hint's album is
/// kept, so a track Deezer filed under an album MusicBrainz lacks stays there.
pub fn metadata_for(recording: &Recording, hint: &TrackHint) -> TrackMetadata {
    let releases: Vec<&ReleaseSummary> = recording.appearances.iter().map(|a| &a.release).collect();
    let mut meta = match pick_release(&releases, &hint.album, &recording.title) {
        Some(index) => recording.appearances[index].metadata.clone(),
        None => TrackMetadata { album: hint.album.clone(), ..recording.base.clone() },
    };
    if meta.artist.is_empty() {
        meta.artist = hint.artist.clone();
        meta.album_artist = hint.artist.clone();
    }
    meta.isrc = hint.isrc.clone().or(meta.isrc);
    meta.duration_ms = meta.duration_ms.or(hint.duration_secs.map(|s| s * 1000));
    meta.track_number = meta.track_number.or(hint.track_number);
    meta.disc_number = meta.disc_number.or(hint.disc_number);
    meta.date = meta.date.or_else(|| hint.release_date.clone());
    meta.cover_art_urls.extend(hint.cover_xl_url.clone());
    meta
}

/// A MusicBrainz recording as a search result, shown with its best release.
pub fn request_for(recording: &Recording) -> TrackRequest {
    let releases: Vec<&ReleaseSummary> = recording.appearances.iter().map(|a| &a.release).collect();
    let release = pick_release(&releases, "", &recording.title).map(|i| releases[i]);
    TrackRequest {
        deezer_id: None,
        mb_recording_id: Some(recording.id.clone()),
        title: recording.title.clone(),
        artist: recording.base.artist.clone(),
        album: release.map(|r| r.title.clone()).unwrap_or_default(),
        isrc: recording.base.isrc.clone(),
        duration_secs: recording.length_ms.map_or(0, |ms| (ms + 500) / 1000),
        track_number: None,
        disc_number: None,
        release_date: release.and_then(|r| r.date.clone()),
        cover_url: release.map(|r| musicbrainz::cover_url(&r.id, 250)),
        cover_xl_url: release.map(|r| musicbrainz::cover_url(&r.id, 1200)),
    }
}

/// Picks the release to tag with, as an index into `releases`. Studio albums
/// beat EPs and singles, so a song picked from its single is filed with its
/// album. Compilations and soundtracks only count when they're the album the
/// user picked. Among equals, the picked album wins, then CD/digital editions
/// (vinyl numbers tracks per side), then the earliest.
///
/// Returns `None` when the recording only appears on compilations the user
/// didn't pick, so the caller keeps the search result's album instead of
/// filing the track under "Various Artists". An artist's own compilation
/// ("Mander House, Vol. 1") still beats a single named after the song.
pub fn pick_release(releases: &[&ReleaseSummary], album_hint: &str, title: &str) -> Option<usize> {
    let hint = normalize(album_hint);
    let matches_hint = |r: &ReleaseSummary| {
        let title = normalize(&r.title);
        !title.is_empty() && (title == hint || hint.starts_with(&format!("{title} ")))
    };
    let score = |r: &ReleaseSummary| -> i64 {
        let mut score = match r.status {
            ReleaseStatus::Official => 30,
            ReleaseStatus::Bootleg => -100,
            ReleaseStatus::Other => 0,
        };
        score += match r.kind {
            ReleaseKind::Album => 40,
            ReleaseKind::Ep => 20,
            ReleaseKind::Single | ReleaseKind::Other => 0,
        };
        if matches_hint(r) {
            // "Stranger in the Alps (Deluxe Edition)" also matches "Stranger in the Alps".
            score += if normalize(&r.title) == hint { 30 } else { 20 };
        } else if r.compilation {
            score -= 60;
        }
        if r.preferred_media {
            score += 8;
        }
        score
    };

    let (index, best) = releases.iter().enumerate().max_by(|(_, a), (_, b)| {
        // Earlier release wins ties (so it's "greater" here).
        score(a).cmp(&score(b)).then_with(|| earlier_date(b.date.as_deref(), a.date.as_deref()))
    })?;
    let hint_is_single = hint == normalize(title);
    let unwanted_compilation = best.compilation
        && !matches_hint(best)
        && !hint.is_empty()
        && (best.various_artists || !hint_is_single);
    (!unwanted_compilation).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(title: &str, status: ReleaseStatus, kind: ReleaseKind, compilation: bool, date: &str) -> ReleaseSummary {
        ReleaseSummary {
            id: format!("{title}-{date}"),
            title: title.into(),
            artist: String::new(),
            date: Some(date.into()),
            track_count: None,
            status,
            kind,
            compilation,
            various_artists: false,
            preferred_media: false,
        }
    }
    use ReleaseKind::{Album, Ep, Single};
    use ReleaseStatus::{Official, Other};

    fn pick<'a>(releases: &'a [ReleaseSummary], hint: &str, title: &str) -> Option<&'a ReleaseSummary> {
        let refs: Vec<&ReleaseSummary> = releases.iter().collect();
        pick_release(&refs, hint, title).map(|i| &releases[i])
    }

    #[test]
    fn prefers_release_matching_album_hint() {
        let releases = [
            release("Motion Sickness", Official, Single, false, "2017-07-19"),
            release("Stranger in the Alps", Official, Album, false, "2017-09-22"),
            release("Rockdelux Vol. 2", Other, Album, true, "2018-02-01"),
        ];
        assert_eq!(pick(&releases, "Stranger in the Alps (Deluxe Edition)", "").unwrap().title, "Stranger in the Alps");
    }

    #[test]
    fn falls_back_to_earliest_official_album() {
        let releases = [
            release("Now 6", Official, Album, true, "2001-01-01"),
            release("Discovery", Official, Album, false, "2001-03-12"),
            release("Discovery", Official, Album, false, "2001-03-07"),
        ];
        assert_eq!(pick(&releases, "", "").unwrap().date.as_deref(), Some("2001-03-07"));
    }

    #[test]
    fn files_singles_with_their_album() {
        let releases = [
            release("Believe", Official, Single, false, "2024-09-27"),
            release("Push Me", Official, Ep, false, "2024-11-15"),
            release("Bubble Love", Official, Album, false, "2024-12-06"),
        ];
        assert_eq!(pick(&releases, "Believe", "Believe").unwrap().title, "Bubble Love");
    }

    #[test]
    fn keeps_a_picked_soundtrack() {
        let releases = [
            release("King's Dead", Official, Single, false, "2018-01-11"),
            release("Black Panther: The Album", Official, Album, true, "2018-02-09"),
        ];
        assert_eq!(pick(&releases, "Black Panther: The Album", "").unwrap().title, "Black Panther: The Album");
    }

    #[test]
    fn skips_compilations_the_user_did_not_pick() {
        let mut various = release("Nova Le Grand Mix 2016", Official, Album, true, "2016-11-18");
        various.various_artists = true;
        assert!(pick(&[various], "Human Energy", "Do It 4 U").is_none());
    }

    #[test]
    fn prefers_the_artists_own_compilation_to_a_single() {
        let own = [release("Mander House, Vol. 1", Official, Album, true, "2018-10-19")];
        assert_eq!(pick(&own, "Oh Lord", "Oh Lord").unwrap().title, "Mander House, Vol. 1");
        assert!(pick(&own, "Some Other Album", "Oh Lord").is_none());
    }

    #[test]
    fn prefers_cd_and_digital_track_numbering_over_vinyl() {
        let vinyl = release("Merriweather Post Pavilion", Official, Album, false, "2009-01-06");
        let mut cd = release("Merriweather Post Pavilion", Official, Album, false, "2009-01-20");
        cd.preferred_media = true;
        assert_eq!(pick(&[vinyl, cd], "Merriweather Post Pavilion", "").unwrap().date.as_deref(), Some("2009-01-20"));
    }

    #[test]
    fn short_titles_only_match_whole_words() {
        let releases = [
            release("I", Official, Album, true, "2010-01-01"),
            release("Imaginal Disk", Official, Album, false, "2024-08-23"),
        ];
        assert_eq!(pick(&releases, "Imaginal Disk", "").unwrap().title, "Imaginal Disk");
    }
}
