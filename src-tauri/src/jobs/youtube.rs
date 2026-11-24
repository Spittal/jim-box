//! The manual fallback for tracks Soulseek doesn't have: the song from
//! YouTube Music, as lossy AAC, tagged and filed like any download. Being
//! lossy, it's upgraded when a later search finds a lossless copy.

use std::collections::HashSet;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use tracing::{info, warn};
use uuid::Uuid;

use super::pipeline::{complete, finish, take_over};
use super::ranking::{version_context, MAX_LENGTH_DIFF_SECS, VERSION_WORDS};
use super::JobStore;
use crate::autotag;
use crate::error::{AppError, AppResult};
use crate::sources::youtube::{YoutubeTrack, YtDlp};
use crate::sources::Sources;
use crate::state::{JobSource, JobStatus, SettingsState, TrackMetadata, TransferProgress, YoutubeMatch};
use crate::text::{normalize, primary_artist, split_version, strip_noise, tokens, version_words};

/// Songs to consider from each search.
const ISRC_RESULTS: usize = 3;
const TEXT_RESULTS: usize = 5;
/// Matches kept on the job to pick from.
const MAX_MATCHES: usize = 5;
/// Matches tried in turn, best first, before giving up.
const MAX_AUTO_ATTEMPTS: usize = 2;

/// What searching YouTube Music and trying its matches came to.
pub(super) enum Outcome {
    /// One was downloaded and filed.
    Done,
    /// Nothing matched the track, and why.
    NoMatch(String),
}

/// Downloads a job's track from YouTube Music: the match at `index`, or
/// when `None` the best ones a search finds, in turn. A job still on
/// Soulseek (searching, waiting to search again, or queued at a peer) is
/// stopped first, its transfer cancelled.
pub async fn start(app: &AppHandle, id: Uuid, index: Option<usize>) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    if job.replaces.is_some() {
        return Err(AppError::Validation(
            "This would replace a file already in your library with a lossy YouTube copy".into(),
        ));
    }
    if index.is_some_and(|i| i >= job.youtube_matches.len()) {
        return Err(AppError::Validation("That YouTube Music song is no longer available".into()));
    }
    if store.is_running(id) && job.source == JobSource::Youtube {
        return Err(AppError::Validation("This download is already running".into()));
    }
    // Fail now, not in the background, if yt-dlp is missing.
    YtDlp::locate(None)?;
    take_over(app, id).await?;
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
        let result = async {
            let meta = identify(&task_app, id).await?;
            match index {
                Some(index) => attempt(&task_app, id, index, &meta).await,
                None => match cascade(&task_app, id, &meta).await? {
                    Outcome::Done => Ok(()),
                    Outcome::NoMatch(why) => Err(AppError::Youtube(why)),
                },
            }
        }
        .await;
        finish(&task_app, id, result);
    })
}

/// The job's metadata, looked up first if it has none yet.
async fn identify(app: &AppHandle, id: Uuid) -> AppResult<TrackMetadata> {
    let store = app.state::<JobStore>();
    let job = store.get(id)?;
    if let Some(meta) = job.metadata {
        return Ok(meta);
    }
    store.update(app, id, |j| j.status = JobStatus::Identifying)?;
    let meta = autotag::resolve(&app.state::<Sources>(), &job.request).await;
    store.update(app, id, |j| j.metadata = Some(meta.clone()))?;
    Ok(meta)
}

/// Searches YouTube Music for the track, keeps its matches on the job, and
/// downloads the best ones in turn until one works. The last step of a
/// download once no Soulseek file has worked, and the "Get from YouTube
/// Music" button.
pub(super) async fn cascade(app: &AppHandle, id: Uuid, meta: &TrackMetadata) -> AppResult<Outcome> {
    let store = app.state::<JobStore>();
    store.update(app, id, |j| {
        j.status = JobStatus::Searching;
        j.source = JobSource::Youtube;
        j.active_candidate = None;
        j.transfer_id = None;
        j.progress = None;
    })?;
    let count = match find(app, id, meta).await? {
        Ok(count) => count,
        Err(why) => return Ok(Outcome::NoMatch(why)),
    };
    let mut last_error = None;
    for index in 0..count.min(MAX_AUTO_ATTEMPTS) {
        match attempt(app, id, index, meta).await {
            Ok(()) => return Ok(Outcome::Done),
            Err(e) => {
                warn!("Job {id}: YouTube Music match {index} failed: {e}");
                last_error = Some(e);
            }
        }
    }
    Err(last_error.expect("at least one match was tried"))
}

/// Searches YouTube Music and keeps the matches, best first, on the job:
/// how many there are, or why none matched.
async fn find(app: &AppHandle, id: Uuid, meta: &TrackMetadata) -> AppResult<Result<usize, String>> {
    let store = app.state::<JobStore>();
    let ytdlp = YtDlp::locate(None)?;
    let job = store.get(id)?;
    let mut ids: Vec<String> = Vec::new();
    let mut isrc_hits = HashSet::new();
    if let Some(isrc) = meta.isrc.as_ref().or(job.request.isrc.as_ref()) {
        for found in ytdlp.search(isrc, ISRC_RESULTS).await? {
            isrc_hits.insert(found.clone());
            ids.push(found);
        }
    }
    let text = search_text(meta);
    for found in ytdlp.search(&text, TEXT_RESULTS).await? {
        if !ids.contains(&found) {
            ids.push(found);
        }
    }
    if ids.is_empty() {
        store.update(app, id, |j| j.youtube_matches.clear())?;
        return Ok(Err(format!("YouTube Music found nothing for “{text}”")));
    }
    let tracks = ytdlp.details(&ids).await?;
    let (matches, rejected) = rank(meta, &tracks, &isrc_hits);
    let found: Vec<YoutubeMatch> = matches
        .iter()
        .take(MAX_MATCHES)
        .map(|(t, _)| YoutubeMatch {
            video_id: t.id.clone(),
            title: t.track.clone().unwrap_or_else(|| t.title.clone()),
            channel: t.channel.clone(),
            url: t.url(),
            length_secs: t.duration_secs(),
            failed: false,
        })
        .collect();
    let count = found.len();
    store.update(app, id, |j| j.youtube_matches = found)?;
    Ok(if count == 0 { Err(no_match_message(&rejected)) } else { Ok(count) })
}

/// Downloads the match at `index`, then tags and files it. Marks the match
/// as failed on error, so the UI can show which were tried.
async fn attempt(app: &AppHandle, id: Uuid, index: usize, meta: &TrackMetadata) -> AppResult<()> {
    let result = download(app, id, index, meta).await;
    if result.is_err() {
        let _ = app.state::<JobStore>().update(app, id, |j| {
            if let Some(m) = j.youtube_matches.get_mut(index) {
                m.failed = true;
            }
            j.progress = None;
        });
    }
    result
}

async fn download(app: &AppHandle, id: Uuid, index: usize, meta: &TrackMetadata) -> AppResult<()> {
    let store = app.state::<JobStore>();
    let settings = app.state::<SettingsState>().snapshot();
    let ytdlp = YtDlp::locate(settings.youtube_cookies_browser.clone())?;
    let job = store.get(id)?;
    let found = job
        .youtube_matches
        .get(index)
        .cloned()
        .ok_or_else(|| AppError::Validation("That YouTube Music song is no longer available".into()))?;
    info!("Job {id}: downloading “{}” ({}) from YouTube Music", found.title, found.url);

    store.update(app, id, |j| {
        j.status = JobStatus::Downloading;
        j.source = JobSource::Youtube;
        j.active_candidate = None;
        j.youtube = Some(found.source());
        j.progress = None;
        j.error = None;
    })?;
    let staging = staging_dir(app, id)?;
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let downloaded = ytdlp
        .download(&found.video_id, &staging, |p| {
            let progress =
                TransferProgress { bytes_transferred: p.bytes, size: p.total, speed: p.speed, place_in_queue: None };
            let _ = store.set_progress(app, id, Some(progress));
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

    let kbps = downloaded.kbps;
    let done = complete(app, id, downloaded.path, meta.clone(), None, move |j| {
        if let Some(yt) = j.youtube.as_mut() {
            yt.bit_rate = kbps;
        }
    })
    .await;
    let _ = std::fs::remove_dir_all(&staging);
    done
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
