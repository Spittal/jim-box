//! The manual fallback for tracks Soulseek doesn't have: the song from
//! YouTube Music, as lossy AAC, tagged and filed like any download. Being
//! lossy, it's upgraded when a later search finds a lossless copy.

use std::collections::HashSet;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use tracing::info;
use uuid::Uuid;

use super::pipeline::{file_into_library, finish};
use super::ranking::{version_context, MAX_LENGTH_DIFF_SECS, VERSION_WORDS};
use super::JobStore;
use crate::autotag;
use crate::error::{AppError, AppResult};
use crate::sources::youtube::{YoutubeTrack, YtDlp};
use crate::sources::Sources;
use crate::state::{JobSource, JobStatus, SettingsState, TrackMetadata, TransferProgress, YoutubeSource};
use crate::text::{normalize, primary_artist, split_version, strip_noise, tokens, version_words};

/// Songs to consider from each search.
const ISRC_RESULTS: usize = 3;
const TEXT_RESULTS: usize = 5;

/// Downloads a job's track from YouTube Music. A job waiting to search
/// Soulseek again stops waiting; other running jobs are left alone.
pub fn start(app: &AppHandle, id: Uuid) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    if store.is_running(id) && job.status != JobStatus::Waiting {
        return Err(AppError::Validation("This download is already running".into()));
    }
    if job.replaces.is_some() {
        return Err(AppError::Validation(
            "This would replace a file already in your library with a lossy YouTube copy".into(),
        ));
    }
    // Fail now, not in the background, if yt-dlp is missing.
    YtDlp::locate(None)?;
    // A waiting job is asleep between searches, holding no slot or transfer.
    store.abort(id);
    store.update(app, id, |j| {
        j.status = JobStatus::Queued;
        j.source = JobSource::Youtube;
        j.youtube = None;
        j.error = None;
        j.active_candidate = None;
        j.transfer_id = None;
        j.progress = None;
        j.retry_at = None;
        j.output_path = None;
        j.already_in_library = false;
    })?;
    let task_app = app.clone();
    store.spawn(id, async move {
        let result = run(&task_app, id).await;
        finish(&task_app, id, result);
    })
}

async fn run(app: &AppHandle, id: Uuid) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let settings = app.state::<SettingsState>().snapshot();
    let ytdlp = YtDlp::locate(settings.youtube_cookies_browser.clone())?;
    let job = store.get(id)?;
    let meta = match job.metadata {
        Some(meta) => meta,
        None => {
            store.update(app, id, |j| j.status = JobStatus::Identifying)?;
            let meta = autotag::resolve(&app.state::<Sources>(), &job.request).await;
            store.update(app, id, |j| j.metadata = Some(meta.clone()))?;
            meta
        }
    };

    store.update(app, id, |j| j.status = JobStatus::Searching)?;
    let mut ids: Vec<String> = Vec::new();
    let mut isrc_hits = HashSet::new();
    if let Some(isrc) = meta.isrc.as_ref().or(job.request.isrc.as_ref()) {
        for found in ytdlp.search(isrc, ISRC_RESULTS).await? {
            isrc_hits.insert(found.clone());
            ids.push(found);
        }
    }
    let text = search_text(&meta);
    for found in ytdlp.search(&text, TEXT_RESULTS).await? {
        if !ids.contains(&found) {
            ids.push(found);
        }
    }
    if ids.is_empty() {
        return Err(AppError::Youtube(format!("YouTube Music found nothing for “{text}”")));
    }
    let tracks = ytdlp.details(&ids).await?;
    let (matches, rejected) = rank(&meta, &tracks, &isrc_hits);
    let Some((best, _)) = matches.into_iter().next() else {
        return Err(AppError::Youtube(no_match_message(&rejected)));
    };
    info!("Job {id}: downloading “{}” ({}) from YouTube Music", best.title, best.url());

    store.update(app, id, |j| {
        j.status = JobStatus::Downloading;
        j.youtube = Some(YoutubeSource {
            video_id: best.id.clone(),
            title: best.track.clone().unwrap_or_else(|| best.title.clone()),
            channel: best.channel.clone(),
            url: best.url(),
            length_secs: best.duration_secs(),
            bit_rate: None,
        });
    })?;
    let staging = staging_dir(app, id)?;
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let downloaded = ytdlp
        .download(&best.id, &staging, |p| {
            let _ = store.update(app, id, |j| {
                j.progress = Some(TransferProgress {
                    bytes_transferred: p.bytes,
                    size: p.total,
                    speed: p.speed,
                    place_in_queue: None,
                })
            });
        })
        .await;
    let downloaded = match downloaded {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    info!("Job {id}: got format {} at {:?} kbps", downloaded.format_id, downloaded.kbps);

    store.update(app, id, |j| {
        j.status = JobStatus::Tagging;
        j.progress = None;
        if let Some(yt) = j.youtube.as_mut() {
            yt.bit_rate = downloaded.kbps;
        }
    })?;
    let dest = file_into_library(app, id, downloaded.path, meta, None).await;
    let _ = std::fs::remove_dir_all(&staging);
    let dest = dest?;
    info!("Job {id} completed from YouTube Music: {}", dest.display());
    let playlist = store.update(app, id, |j| {
        j.status = JobStatus::Completed;
        j.output_path = Some(dest.clone());
        j.playlist.clone()
    })?;
    let arrival = crate::rekordbox::Arrival { path: dest, replaces: None, playlist };
    crate::ingest::arrived(app, vec![arrival]).await;
    Ok(())
}

fn staging_dir(app: &AppHandle, id: Uuid) -> AppResult<PathBuf> {
    Ok(app.path().app_data_dir()?.join("youtube").join(id.to_string()))
}

/// "ariana grande thank u, next": the lead artist and the title with its
/// version but without "(feat. …)" and other noise.
fn search_text(meta: &TrackMetadata) -> String {
    format!("{} {}", primary_artist(&meta.artist), strip_noise(&meta.title)).trim().to_string()
}

/// An upload that passed every check, with its score.
type Scored = (YoutubeTrack, i64);
/// An upload that failed a check, with the reason.
type Rejected = (YoutubeTrack, String);

/// The uploads that are the requested recording, best first, and the others
/// with the reason they were turned down.
pub fn rank(
    meta: &TrackMetadata,
    tracks: &[YoutubeTrack],
    isrc_hits: &HashSet<String>,
) -> (Vec<Scored>, Vec<Rejected>) {
    let (name, _) = split_version(&meta.title);
    let title_tokens = tokens(&name);
    let version = version_words(&meta.title);
    let requested: HashSet<String> = tokens(&meta.title).into_iter().collect();
    let artist = normalize(primary_artist(&meta.artist)).replace(' ', "");
    let album = normalize(&strip_noise(&meta.album));
    let target_secs = meta.duration_ms.map(|ms| (ms + 500) / 1000);

    let mut matches = Vec::new();
    let mut rejected = Vec::new();
    for track in tracks {
        let shown = track.track.as_deref().unwrap_or(&track.title);
        let check = || -> Result<i64, String> {
            let shown_tokens: HashSet<String> = tokens(shown).into_iter().collect();
            if !title_tokens.iter().all(|t| shown_tokens.contains(t)) {
                return Err("Different title".into());
            }
            // Songs credit their artists; covers often name the original
            // artist in the title ("Originally Performed By …"), so only
            // videos without credits are judged by their title.
            let credits = match &track.artists {
                Some(artists) if !artists.is_empty() => {
                    format!("{} {}", artists.join(" "), track.channel.as_deref().unwrap_or_default())
                }
                _ => format!("{} {}", track.channel.as_deref().unwrap_or_default(), track.title),
            };
            if !artist.is_empty() && !normalize(&credits).replace(' ', "").contains(&artist) {
                return Err(format!("Not by {}", primary_artist(&meta.artist)));
            }
            let compact = normalize(shown).replace(' ', "");
            if !version.iter().all(|w| compact.contains(w.as_str())) {
                return Err(format!("Not the “{}” version", version.join(" ")));
            }
            let context: HashSet<String> = version_context(shown).into_iter().collect();
            if let Some(word) = VERSION_WORDS.iter().find(|w| context.contains(**w) && !requested.contains(**w)) {
                return Err(format!("Looks like a different version (“{word}”)"));
            }
            let mut score = 0;
            match (target_secs, track.duration_secs()) {
                (Some(target), Some(len)) if len.abs_diff(target) > MAX_LENGTH_DIFF_SECS => {
                    return Err(format!("Length {}, expected {}", mmss(len), mmss(target)));
                }
                (Some(target), Some(len)) => score -= 2 * len.abs_diff(target) as i64,
                _ => score -= 10,
            }
            // YouTube Music songs, rather than videos, carry track metadata.
            if track.track.is_some() {
                score += 30;
            }
            if !album.is_empty() && track.album.as_deref().is_some_and(|a| normalize(&strip_noise(a)) == album) {
                score += 15;
            }
            if isrc_hits.contains(&track.id) {
                score += 20;
            }
            Ok(score)
        };
        match check() {
            Ok(score) => matches.push((track.clone(), score)),
            Err(reason) => rejected.push((track.clone(), reason)),
        }
    }
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    (matches, rejected)
}

fn no_match_message(rejected: &[Rejected]) -> String {
    let examples: Vec<String> = rejected
        .iter()
        .take(3)
        .map(|(t, reason)| format!("“{}”: {reason}", t.track.as_deref().unwrap_or(&t.title)))
        .collect();
    if examples.is_empty() {
        "No YouTube Music song matched".into()
    } else {
        format!("No YouTube Music song matched. {}", examples.join("; "))
    }
}

fn mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> TrackMetadata {
        TrackMetadata {
            title: "thank u, next".into(),
            artist: "Ariana Grande".into(),
            album: "thank u, next".into(),
            album_artist: "Ariana Grande".into(),
            duration_ms: Some(207_333),
            isrc: Some("USUM71819361".into()),
            ..Default::default()
        }
    }

    fn song(id: &str, track: &str, artist: &str, album: &str, secs: f64) -> YoutubeTrack {
        YoutubeTrack {
            id: id.into(),
            title: track.into(),
            track: Some(track.into()),
            artists: Some(vec![artist.into()]),
            album: Some(album.into()),
            duration: Some(secs),
            channel: Some(artist.into()),
        }
    }

    fn video(id: &str, title: &str, channel: &str, secs: f64) -> YoutubeTrack {
        YoutubeTrack {
            id: id.into(),
            title: title.into(),
            duration: Some(secs),
            channel: Some(channel.into()),
            ..Default::default()
        }
    }

    fn ids(matches: &[Scored]) -> Vec<&str> {
        matches.iter().map(|(t, _)| t.id.as_str()).collect()
    }

    #[test]
    fn searches_for_artist_and_title() {
        let mut m = meta();
        m.artist = "Calvin Harris feat. Rihanna".into();
        m.title = "This Is What You Came For (feat. Rihanna) [Remastered]".into();
        assert_eq!(search_text(&m), "Calvin Harris This Is What You Came For");
    }

    #[test]
    fn prefers_the_song_over_videos_and_other_versions() {
        let tracks = vec![
            video("mv", "Ariana Grande - thank u, next (Official Video)", "Ariana Grande", 329.0),
            song("live", "thank u, next (live)", "Ariana Grande", "k bye for now (swt live)", 207.0),
            video("audio", "Ariana Grande - thank u, next (Audio)", "Ariana Grande", 208.0),
            song("song", "thank u, next", "Ariana Grande", "thank u, next", 207.0),
            song("cover", "thank u, next", "Some Covers Band", "Pop Hits", 207.0),
            song("karaoke", "thank u, next (Originally Performed By Ariana Grande)", "ZZang KARAOKE", "Hits", 207.0),
            song("bebe", "BEBE", "6ix9ine", "DUMMY BOY", 218.0),
        ];
        let (matches, rejected) = rank(&meta(), &tracks, &HashSet::from(["song".to_string()]));
        assert_eq!(ids(&matches), vec!["song", "audio"]);
        let reasons: Vec<(&str, &str)> = rejected.iter().map(|(t, r)| (t.id.as_str(), r.as_str())).collect();
        assert_eq!(
            reasons,
            vec![
                ("mv", "Length 5:29, expected 3:27"),
                ("live", "Looks like a different version (“live”)"),
                ("cover", "Not by Ariana Grande"),
                ("karaoke", "Not by Ariana Grande"),
                ("bebe", "Different title"),
            ]
        );
    }

    #[test]
    fn requires_requested_versions() {
        let mut m = meta();
        m.title = "thank u, next (Remix)".into();
        let tracks = vec![song("a", "thank u, next", "Ariana Grande", "thank u, next", 207.0)];
        let (matches, rejected) = rank(&m, &tracks, &HashSet::new());
        assert!(matches.is_empty());
        assert_eq!(rejected[0].1, "Not the “remix” version");
    }

    #[test]
    fn explains_when_nothing_matched() {
        let rejected = vec![(video("x", "Other Song", "Someone", 100.0), "Different title".to_string())];
        assert_eq!(no_match_message(&rejected), "No YouTube Music song matched. “Other Song”: Different title");
    }

    /// Searches YouTube Music and downloads the best match into a temporary
    /// folder, then tags it like the app would:
    /// `cargo test live_youtube -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_youtube_download() {
        let ytdlp = YtDlp::locate(None).expect("yt-dlp installed");
        println!("{:?}", ytdlp.info().await.unwrap());
        let m = meta();
        let mut ids = ytdlp.search(m.isrc.as_deref().unwrap(), ISRC_RESULTS).await.unwrap();
        let isrc_hits: HashSet<String> = ids.iter().cloned().collect();
        for id in ytdlp.search(&search_text(&m), TEXT_RESULTS).await.unwrap() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let tracks = ytdlp.details(&ids).await.unwrap();
        let (matches, rejected) = rank(&m, &tracks, &isrc_hits);
        for (t, score) in &matches {
            println!("ok {score:>4} {} | {:?} | {:?}", t.id, t.track.as_deref().unwrap_or(&t.title), t.album);
        }
        for (t, reason) in &rejected {
            println!("no      {} | {} | {reason}", t.id, t.track.as_deref().unwrap_or(&t.title));
        }
        let best = &matches.first().expect("a match").0;
        assert_eq!(best.track.as_deref(), Some("thank u, next"));

        let dir = std::env::temp_dir().join(format!("jim-box-yt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut reports = 0;
        let d = ytdlp.download(&best.id, &dir, |_| reports += 1).await.unwrap();
        println!("{d:?} after {reports} progress reports");
        assert!(reports > 0);
        crate::library::write_tags(&d.path, &m, None, crate::library::TagMode::Replace).unwrap();
        let read = crate::library::read_file(&d.path).unwrap();
        println!("tagged: {read:?}");
        assert!(!read.lossless);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
