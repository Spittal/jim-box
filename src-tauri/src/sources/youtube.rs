//! YouTube Music, through the yt-dlp command-line tool. Jim Box uses the
//! copy installed with a package manager (Homebrew on macOS, winget or Scoop
//! on Windows), which keeps yt-dlp and the JavaScript runtime YouTube needs
//! up to date; YouTube changes often enough that a copy bundled with the app
//! would soon stop working.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::error::{AppError, AppResult};

const BINARY_NAME: &str = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
const INSTALL_HINT: &str = if cfg!(windows) {
    "yt-dlp isn't installed. Install it with winget (winget install yt-dlp), then reopen Jim Box"
} else {
    "yt-dlp isn't installed. Install it with Homebrew: brew install yt-dlp"
};
const SEARCH_TIMEOUT: Duration = Duration::from_secs(90);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// AAC only, since rekordbox and most DJ software can't play Opus: 141 is
/// the 256 kbps stream YouTube Music Premium accounts get, 140 the 128 kbps
/// one anyone gets.
const FORMAT: &str = "141/140/bestaudio[ext=m4a]";
/// How often download progress is reported.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Browsers yt-dlp can read YouTube cookies from, for Premium's 256 kbps stream.
#[cfg(target_os = "macos")]
pub const COOKIE_BROWSERS: &[&str] = &["safari", "chrome", "edge", "firefox", "brave", "vivaldi"];
#[cfg(not(target_os = "macos"))]
pub const COOKIE_BROWSERS: &[&str] = &["chrome", "edge", "firefox", "brave", "vivaldi"];

/// An upload, as yt-dlp describes it. Songs on YouTube Music have `track`,
/// `artists` and `album`; ordinary videos only a title and channel.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct YoutubeTrack {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub artists: Option<Vec<String>>,
    #[serde(default)]
    pub album: Option<String>,
    /// Seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub channel: Option<String>,
}

impl YoutubeTrack {
    pub fn duration_secs(&self) -> Option<u32> {
        self.duration.map(|d| d.round() as u32)
    }

    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.id)
    }
}

/// The installed yt-dlp, for the settings page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtDlpInfo {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DownloadProgress {
    pub bytes: u64,
    pub total: u64,
    /// Bytes per second.
    pub speed: f64,
}

#[derive(Debug, Clone)]
pub struct Downloaded {
    pub path: PathBuf,
    /// yt-dlp's format code: "141" for 256 kbps AAC, "140" for 128 kbps.
    pub format_id: String,
    /// Average bitrate in kbps, as YouTube reports it.
    pub kbps: Option<u32>,
}

pub struct YtDlp {
    binary: PathBuf,
    /// Browser to take YouTube cookies from when downloading.
    cookies_browser: Option<String>,
}

impl YtDlp {
    /// Finds yt-dlp, or explains how to install it.
    pub fn locate(cookies_browser: Option<String>) -> AppResult<Self> {
        let binary = find_binary().ok_or_else(|| AppError::Youtube(INSTALL_HINT.into()))?;
        Ok(Self { binary, cookies_browser })
    }

    pub async fn info(&self) -> AppResult<YtDlpInfo> {
        let out = self.run(&["--version"], SEARCH_TIMEOUT).await?;
        Ok(YtDlpInfo { path: self.binary.clone(), version: out.trim().to_string() })
    }

    /// Ids of the first `limit` songs YouTube Music finds for `query`.
    pub async fn search(&self, query: &str, limit: usize) -> AppResult<Vec<String>> {
        let mut url = reqwest::Url::parse("https://music.youtube.com/search").expect("valid URL");
        url.query_pairs_mut().append_pair("q", query);
        url.set_fragment(Some("songs"));
        let range = format!("1:{limit}");
        let out = self
            .run(&["--flat-playlist", "-I", &range, "--print", "id", url.as_str()], SEARCH_TIMEOUT)
            .await?;
        Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
    }

    /// Full details of each upload. Unavailable ones are left out.
    pub async fn details(&self, ids: &[String]) -> AppResult<Vec<YoutubeTrack>> {
        let urls: Vec<String> = ids.iter().map(|id| format!("https://music.youtube.com/watch?v={id}")).collect();
        let mut args = vec!["-j", "--no-playlist", "--ignore-errors"];
        args.extend(urls.iter().map(String::as_str));
        let out = self.run(&args, SEARCH_TIMEOUT).await?;
        Ok(out.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }

    /// Downloads an upload's AAC audio into `dir`, as is (no re-encoding).
    pub async fn download(
        &self,
        id: &str,
        dir: &Path,
        mut on_progress: impl FnMut(DownloadProgress),
    ) -> AppResult<Downloaded> {
        let output = dir.join("%(id)s.%(ext)s");
        let url = format!("https://music.youtube.com/watch?v={id}");
        let mut cmd = self.command();
        cmd.args(["--no-playlist", "--no-warnings", "-f", FORMAT, "-o"])
            .arg(&output)
            .args([
                "--newline",
                "--progress",
                "--progress-template",
                "download:TFPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s \
                 %(progress.total_bytes_estimate)s %(progress.speed)s",
                "--no-simulate",
                "-O",
                "after_move:TFDONE %(format_id)s %(abr)s %(filepath)s",
            ]);
        if let Some(browser) = &self.cookies_browser {
            cmd.args(["--cookies-from-browser", browser]);
        }
        cmd.arg(&url).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });

        let follow = async {
            let mut lines = BufReader::new(stdout).lines();
            let mut done = None;
            let mut last_report: Option<Instant> = None;
            while let Some(line) = lines.next_line().await? {
                if let Some(progress) = parse_progress(&line) {
                    let finished = progress.total > 0 && progress.bytes >= progress.total;
                    if finished || last_report.map_or(true, |t| t.elapsed() >= PROGRESS_INTERVAL) {
                        on_progress(progress);
                        last_report = Some(Instant::now());
                    }
                } else if let Some(d) = parse_done(&line) {
                    done = Some(d);
                }
            }
            let status = child.wait().await?;
            AppResult::Ok((status, done))
        };
        let (status, done) = tokio::time::timeout(DOWNLOAD_TIMEOUT, follow)
            .await
            .map_err(|_| AppError::Youtube("The download timed out".into()))??;
        let stderr = errors.await.unwrap_or_default();
        match done {
            Some(d) if status.success() && d.path.is_file() => Ok(d),
            _ => Err(AppError::Youtube(error_message(&stderr, "The download failed"))),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        crate::process::hide_console(&mut cmd);
        // Windows would otherwise print file names in the system code page,
        // which isn't the UTF-8 the output is read as.
        if cfg!(windows) {
            cmd.args(["--encoding", "utf-8"]);
        }
        // yt-dlp needs Deno (or Node) for YouTube and uses ffmpeg to tidy up
        // downloaded files; both live next to it.
        let mut dirs: Vec<PathBuf> = self.binary.parent().map(Path::to_path_buf).into_iter().collect();
        dirs.extend(tool_dirs());
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        if let Ok(path) = std::env::join_paths(dirs) {
            cmd.env("PATH", path);
        }
        cmd.stdin(Stdio::null()).kill_on_drop(true);
        cmd
    }

    /// Runs yt-dlp to completion and returns its standard output.
    async fn run(&self, args: &[&str], limit: Duration) -> AppResult<String> {
        let output = tokio::time::timeout(limit, self.command().args(args).output())
            .await
            .map_err(|_| AppError::Youtube("YouTube took too long to answer".into()))??;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        // With --ignore-errors, yt-dlp fails if any one upload did, so only
        // treat it as failing when it produced nothing.
        if !output.status.success() && stdout.trim().is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Youtube(error_message(&stderr, "yt-dlp failed")));
        }
        Ok(stdout)
    }
}

fn find_binary() -> Option<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    tool_dirs()
        .into_iter()
        .chain(path_dirs)
        .map(|dir| dir.join(BINARY_NAME))
        .find(|path| path.is_file())
}

/// Where package managers install command-line tools. Apps opened from the
/// Finder don't get the shell's PATH, so these are searched explicitly.
#[cfg(not(windows))]
fn tool_dirs() -> Vec<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].iter().map(PathBuf::from).collect()
}

/// Where winget, Scoop and Chocolatey put command-line tools. They're on the
/// PATH too, but only for apps started after the install, so these are
/// searched explicitly: yt-dlp is then found without logging out and back in.
#[cfg(windows)]
fn tool_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    [
        var("LOCALAPPDATA").map(|dir| dir.join("Microsoft").join("WinGet").join("Links")),
        var("USERPROFILE").map(|dir| dir.join("scoop").join("shims")),
        var("ProgramData").map(|dir| dir.join("chocolatey").join("bin")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn parse_progress(line: &str) -> Option<DownloadProgress> {
    let mut fields = line.strip_prefix("TFPROG ")?.split(' ');
    let mut next = || fields.next().and_then(|f| f.parse::<f64>().ok());
    let bytes = next()?;
    let (total, estimate, speed) = (next(), next(), next());
    Some(DownloadProgress {
        bytes: bytes as u64,
        total: total.or(estimate).unwrap_or_default() as u64,
        speed: speed.unwrap_or_default(),
    })
}

fn parse_done(line: &str) -> Option<Downloaded> {
    let mut fields = line.strip_prefix("TFDONE ")?.splitn(3, ' ');
    let format_id = fields.next()?.to_string();
    let kbps = fields.next()?.parse::<f64>().ok().map(|k| k.round() as u32);
    let path = PathBuf::from(fields.next()?);
    Some(Downloaded { path, format_id, kbps })
}

/// yt-dlp's last error line, without its "ERROR: [youtube] id:" prefix.
fn error_message(stderr: &str, fallback: &str) -> String {
    stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("ERROR: "))
        .map(|e| {
            let e = e.trim();
            // "[youtube] GftZ601-pXs: Sign in to confirm…" → "Sign in to confirm…"
            match e.strip_prefix('[').and_then(|rest| rest.split_once(": ")) {
                Some((_, message)) => message.to_string(),
                None => e.to_string(),
            }
        })
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        let p = parse_progress("TFPROG 3072 3358309 NA 2603536.4").unwrap();
        assert_eq!((p.bytes, p.total, p.speed as u64), (3072, 3358309, 2603536));
        let p = parse_progress("TFPROG 1024 NA 5000000 NA").unwrap();
        assert_eq!((p.bytes, p.total, p.speed as u64), (1024, 5000000, 0));
        assert!(parse_progress("[download] 10%").is_none());
    }

    #[test]
    fn parses_the_finished_file() {
        let d = parse_done("TFDONE 140 129.59 /tmp/My Music/GftZ601-pXs.m4a").unwrap();
        assert_eq!(d.format_id, "140");
        assert_eq!(d.kbps, Some(130));
        assert_eq!(d.path, PathBuf::from("/tmp/My Music/GftZ601-pXs.m4a"));
    }

    #[test]
    fn extracts_error_messages() {
        let stderr = "WARNING: something\nERROR: [youtube] GftZ601-pXs: Sign in to confirm your age\n";
        assert_eq!(error_message(stderr, "x"), "Sign in to confirm your age");
        assert_eq!(error_message("ERROR: Requested format is not available", "x"), "Requested format is not available");
        assert_eq!(error_message("", "fallback"), "fallback");
    }
}
