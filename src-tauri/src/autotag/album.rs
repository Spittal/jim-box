//! Album-level matching, after beets: score how well a folder of files fits a
//! release as a whole, then assign each file to one of its tracks. Matching
//! albums rather than single tracks keeps a folder together on one edition
//! and uses track order and lengths as evidence.

use std::path::Path;

use super::distance::{artist_distance, earlier_date, majority, string_distance};
use super::hint::hint_from_file;
use crate::sources::musicbrainz::{AlbumRelease, AlbumTrack, ReleaseStatus, ReleaseSummary};
use crate::state::{Confidence, ExistingTags};

/// Below this distance a match is applied by default (beets' "strong" match).
const STRONG_MATCH: f64 = 0.12;
/// Below this a match is proposed for review; above, the folder falls back
/// to matching track by track.
const ACCEPTABLE_MATCH: f64 = 0.3;
/// A file only gets a track this close to it…
const MAX_TRACK_COST: f64 = 0.5;
/// …whose title shares most of its words, so a duplicate file can't take a
/// different song just because its length and number fit.
const MAX_TITLE_DISTANCE: f64 = 0.5;
/// Releases whose full tracklist is fetched and scored per folder.
pub const MAX_CANDIDATES: usize = 3;

/// What a file's tags (or path) say about it.
#[derive(Debug, Clone, Default)]
pub struct FileTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration_secs: Option<u32>,
    pub mb_recording_id: Option<String>,
    pub mb_release_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlbumMatch {
    /// 0 is a perfect match, 1 no resemblance.
    pub distance: f64,
    /// For each file, the index of its track in the release.
    pub assignment: Vec<Option<usize>>,
}

impl FileTrack {
    /// What a file says about itself: its tags, or its path when untagged.
    pub fn from_file(tags: &ExistingTags, relative_path: &Path, duration_secs: Option<u32>) -> Self {
        let hint = hint_from_file(tags, relative_path, duration_secs).unwrap_or_default();
        Self {
            title: hint.title,
            artist: hint.artist,
            album: hint.album,
            album_artist: tags.album_artist.clone().unwrap_or_default(),
            track_number: tags.track_number,
            disc_number: tags.disc_number,
            duration_secs,
            mb_recording_id: tags.mb_recording_id.clone(),
            mb_release_id: tags.mb_release_id.clone(),
        }
    }

    /// The album artist, or the track artist when there's none.
    pub fn credited_artist(&self) -> &str {
        if self.album_artist.is_empty() { &self.artist } else { &self.album_artist }
    }
}

impl AlbumMatch {
    pub fn matched(&self) -> usize {
        self.assignment.iter().flatten().count()
    }
}

/// How sure a match is, or `None` when it's too far off or leaves more than
/// half the files without a track.
pub fn confidence(found: &AlbumMatch, files: usize) -> Option<Confidence> {
    let enough = found.matched() * 2 >= files.max(2);
    if !enough || found.distance > ACCEPTABLE_MATCH {
        return None;
    }
    Some(if found.distance <= STRONG_MATCH { Confidence::High } else { Confidence::Medium })
}

/// Orders search results by how likely they are to be the folder's release,
/// judged from their summaries, so only the best few tracklists are fetched.
pub fn rank_candidates(candidates: Vec<ReleaseSummary>, files: &[FileTrack]) -> Vec<ReleaseSummary> {
    let album = majority(files.iter().map(|f| f.album.as_str())).unwrap_or_default();
    let tagged_release = majority(files.iter().filter_map(|f| f.mb_release_id.as_deref()));
    let score = |r: &ReleaseSummary| -> f64 {
        let mut distance = string_distance(album, &r.title);
        if r.track_count.is_some_and(|n| (n as usize) < files.len()) {
            distance += 1.0; // Can't hold every file.
        }
        if r.compilation && distance > 0.0 {
            distance += 0.5;
        }
        if r.status != ReleaseStatus::Official {
            distance += 0.2;
        }
        if !r.preferred_media {
            distance += 0.1;
        }
        if tagged_release == Some(r.id.as_str()) {
            distance -= 2.0;
        }
        distance
    };

    let mut unique: Vec<ReleaseSummary> = Vec::new();
    for candidate in candidates {
        if !unique.iter().any(|r| r.id == candidate.id) {
            unique.push(candidate);
        }
    }
    unique.sort_by(|a, b| {
        score(a).total_cmp(&score(b)).then_with(|| earlier_date(a.date.as_deref(), b.date.as_deref()))
    });
    unique
}

/// Scores `files` against `release` and assigns each file its closest track.
pub fn score(files: &[FileTrack], release: &AlbumRelease) -> AlbumMatch {
    let assignment = assign(files, &release.tracks);

    let album = majority(files.iter().map(|f| f.album.as_str())).unwrap_or_default();
    let artist = majority(files.iter().map(FileTrack::credited_artist)).unwrap_or_default();
    let summary = &release.summary;

    // (distance, weight) pairs, as in beets' `Distance`.
    let mut parts: Vec<(f64, f64)> = vec![
        (string_distance(album, &summary.title), 3.0),
        (if summary.compilation { 0.3 } else { artist_distance(artist, &summary.artist) }, 3.0),
    ];
    let costs: Vec<f64> = files
        .iter()
        .zip(&assignment)
        .filter_map(|(file, track)| track.map(|t| track_cost(file, &release.tracks[t])))
        .collect();
    if !costs.is_empty() {
        parts.push((costs.iter().sum::<f64>() / costs.len() as f64, 6.0));
    }
    let unmatched_files = assignment.iter().filter(|t| t.is_none()).count();
    parts.push((ratio(unmatched_files, files.len()), 4.0));
    let missing_tracks = release.tracks.len().saturating_sub(costs.len());
    parts.push((ratio(missing_tracks, release.tracks.len()), 1.0));
    if let Some(tagged) = majority(files.iter().filter_map(|f| f.mb_release_id.as_deref())) {
        parts.push((if tagged == summary.id { 0.0 } else { 1.0 }, 5.0));
    }
    parts.push((if summary.status == ReleaseStatus::Official { 0.0 } else { 1.0 }, 0.5));
    parts.push((if summary.preferred_media { 0.0 } else { 1.0 }, 0.5));

    let weight: f64 = parts.iter().map(|(_, w)| w).sum();
    let distance = parts.iter().map(|(d, w)| d * w).sum::<f64>() / weight;
    AlbumMatch { distance, assignment }
}

/// Picks the better of two scored releases: lower distance, then earlier.
pub fn better(a: (&AlbumRelease, &AlbumMatch), b: (&AlbumRelease, &AlbumMatch)) -> bool {
    match a.1.distance.total_cmp(&b.1.distance) {
        std::cmp::Ordering::Equal => {
            earlier_date(a.0.summary.date.as_deref(), b.0.summary.date.as_deref()) == std::cmp::Ordering::Less
        }
        order => order == std::cmp::Ordering::Less,
    }
}

/// Greedily pairs files with tracks, cheapest pairs first.
fn assign(files: &[FileTrack], tracks: &[AlbumTrack]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f64, usize, usize)> = files
        .iter()
        .enumerate()
        .flat_map(|(f, file)| tracks.iter().enumerate().map(move |(t, track)| (f, t, file, track)))
        .filter(|(_, _, file, track)| {
            same_recording(file, track) || string_distance(&file.title, &track.title) <= MAX_TITLE_DISTANCE
        })
        .map(|(f, t, file, track)| (track_cost(file, track), f, t))
        .filter(|(cost, _, _)| *cost <= MAX_TRACK_COST)
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut assignment = vec![None; files.len()];
    let mut taken = vec![false; tracks.len()];
    for (_, f, t) in pairs {
        if assignment[f].is_none() && !taken[t] {
            assignment[f] = Some(t);
            taken[t] = true;
        }
    }
    assignment
}

/// How unlike a track a file is, from 0 to 1.
fn track_cost(file: &FileTrack, track: &AlbumTrack) -> f64 {
    if same_recording(file, track) {
        return 0.0;
    }
    let title = string_distance(&file.title, &track.title);
    let length = match (file.duration_secs, track.length_secs) {
        (Some(a), Some(b)) => (a.abs_diff(b) as f64 / 10.0).min(1.0),
        _ => 0.3,
    };
    let index = match file.track_number {
        Some(n) => {
            let same_disc = file.disc_number.map_or(true, |d| d == track.disc);
            if n == track.position && same_disc { 0.0 } else { 1.0 }
        }
        None => 0.3,
    };
    let artist = if file.artist.is_empty() { 0.3 } else { artist_distance(&file.artist, &track.metadata.artist) };
    (3.0 * title + 2.0 * length + index + artist) / 7.0
}

fn same_recording(file: &FileTrack, track: &AlbumTrack) -> bool {
    file.mb_recording_id.is_some() && file.mb_recording_id == track.metadata.mb_recording_id
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 / whole as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TrackMetadata;

    fn summary(id: &str, title: &str, tracks: u32) -> ReleaseSummary {
        ReleaseSummary {
            id: id.into(),
            title: title.into(),
            artist: "Animal Collective".into(),
            date: Some("2009-01-20".into()),
            track_count: Some(tracks),
            status: ReleaseStatus::Official,
            kind: crate::sources::musicbrainz::ReleaseKind::Album,
            compilation: false,
            various_artists: false,
            preferred_media: true,
        }
    }

    fn release(id: &str, title: &str, names: &[(&str, u32)]) -> AlbumRelease {
        let tracks = names
            .iter()
            .enumerate()
            .map(|(i, (name, secs))| AlbumTrack {
                disc: 1,
                position: i as u32 + 1,
                title: name.to_string(),
                length_secs: Some(*secs),
                metadata: TrackMetadata {
                    title: name.to_string(),
                    artist: "Animal Collective".into(),
                    mb_recording_id: Some(format!("rec-{name}")),
                    ..Default::default()
                },
            })
            .collect();
        AlbumRelease { summary: summary(id, title, names.len() as u32), tracks }
    }

    fn file(title: &str, number: u32, secs: u32) -> FileTrack {
        FileTrack {
            title: title.into(),
            artist: "Animal Collective".into(),
            album: "Merriweather Post Pavilion".into(),
            track_number: Some(number),
            duration_secs: Some(secs),
            ..Default::default()
        }
    }

    const MPP: [(&str, u32); 4] =
        [("In the Flowers", 322), ("My Girls", 340), ("Also Frightened", 316), ("Summertime Clothes", 272)];

    #[test]
    fn matches_a_complete_album_strongly() {
        let files: Vec<FileTrack> = MPP.iter().enumerate().map(|(i, (t, s))| file(t, i as u32 + 1, *s)).collect();
        let m = score(&files, &release("mpp", "Merriweather Post Pavilion", &MPP));
        assert!(m.distance < STRONG_MATCH, "distance {}", m.distance);
        assert_eq!(m.assignment, vec![Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn assigns_by_title_and_length_despite_wrong_numbers() {
        let files = vec![file("My Girls", 7, 340), file("In the Flowers", 9, 322)];
        let m = score(&files, &release("mpp", "Merriweather Post Pavilion", &MPP));
        assert_eq!(m.assignment, vec![Some(1), Some(0)]);
        assert!(m.distance < ACCEPTABLE_MATCH, "distance {}", m.distance);
    }

    #[test]
    fn duplicates_do_not_take_other_songs() {
        let files = vec![file("My Girls", 2, 340), file("My Girls", 3, 316)];
        let m = score(&files, &release("mpp", "Merriweather Post Pavilion", &MPP));
        assert_eq!(m.assignment, vec![Some(1), None]);
    }

    #[test]
    fn partial_albums_still_match() {
        let files = vec![file("My Girls", 2, 340)];
        let m = score(&files, &release("mpp", "Merriweather Post Pavilion", &MPP));
        assert!(m.distance < ACCEPTABLE_MATCH, "distance {}", m.distance);
    }

    #[test]
    fn rejects_a_different_album() {
        let files: Vec<FileTrack> = MPP.iter().enumerate().map(|(i, (t, s))| file(t, i as u32 + 1, *s)).collect();
        let other = release("sj", "Strawberry Jam", &[("Peacebone", 312), ("Unsolved Mysteries", 264)]);
        let m = score(&files, &other);
        assert!(m.distance > ACCEPTABLE_MATCH, "distance {}", m.distance);
        assert_eq!(m.matched(), 0);
    }

    #[test]
    fn prefers_the_edition_the_files_fit() {
        let files: Vec<FileTrack> = MPP.iter().enumerate().map(|(i, (t, s))| file(t, i as u32 + 1, *s)).collect();
        let standard = release("std", "Merriweather Post Pavilion", &MPP);
        let mut deluxe_tracks = MPP.to_vec();
        deluxe_tracks.extend([("Bonus One", 200), ("Bonus Two", 210), ("Bonus Three", 220), ("Bonus Four", 230)]);
        let deluxe = release("dlx", "Merriweather Post Pavilion (Deluxe)", &deluxe_tracks);
        let (a, b) = (score(&files, &standard), score(&files, &deluxe));
        assert!(better((&standard, &a), (&deluxe, &b)));
    }

    #[test]
    fn ranks_summaries_that_can_hold_the_files_first() {
        let files: Vec<FileTrack> = MPP.iter().enumerate().map(|(i, (t, s))| file(t, i as u32 + 1, *s)).collect();
        let mut single = summary("single", "My Girls", 1);
        single.date = Some("2008-01-01".into());
        let mut compilation = summary("comp", "Now 72", 40);
        compilation.compilation = true;
        let ranked = rank_candidates(
            vec![single, compilation, summary("mpp", "Merriweather Post Pavilion", 11)],
            &files,
        );
        assert_eq!(ranked[0].id, "mpp");
    }
}
