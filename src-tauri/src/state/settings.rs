use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

use super::TrackMetadata;


/// Formats the app knows how to find and tag. Lossy ones are only a fallback
/// for tracks with no lossless copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Flac,
    Alac,
    Aiff,
    Wav,
    Mp3,
    Aac,
    Vorbis,
    Opus,
}

impl AudioFormat {
    /// The lossless formats, which users choose between in settings.
    pub const ALL: [AudioFormat; 4] = [Self::Flac, Self::Alac, Self::Aiff, Self::Wav];

    pub fn is_lossless(self) -> bool {
        Self::ALL.contains(&self)
    }

    /// Tie-breaker between otherwise equal candidates: better metadata support wins.
    pub fn preference(self) -> i64 {
        match self {
            Self::Flac => 20,
            Self::Alac => 15,
            Self::Aiff => 10,
            Self::Wav | Self::Mp3 => 0,
            Self::Aac | Self::Opus => -5,
            Self::Vorbis => -10,
        }
    }
}

/// Where files go in the music folder, as in beets' `paths` setting: one
/// template for album tracks, one for compilations, one for tracks with no
/// album. Templates use beets syntax (`$albumartist`, `${album}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathTemplates {
    pub default: String,
    pub comp: String,
    pub singleton: String,
}

impl Default for PathTemplates {
    /// Beets' defaults, without `%aunique{}`.
    fn default() -> Self {
        Self {
            default: "$albumartist/$album/$track $title".into(),
            comp: "Compilations/$album/$track $title".into(),
            singleton: "Non-Album/$artist/$title".into(),
        }
    }
}

impl PathTemplates {
    /// The template beets would file this track with.
    pub fn for_track(&self, meta: &TrackMetadata) -> &str {
        if meta.album.trim().is_empty() {
            &self.singleton
        } else if meta.compilation {
            &self.comp
        } else {
            &self.default
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Root of the organized library; finished downloads are moved here.
    pub music_dir: PathBuf,
    /// How organized files are laid out in `music_dir`.
    #[serde(default)]
    pub paths: PathTemplates,
    /// The single template of older versions, in `{field}` syntax; migrated
    /// into `paths.default` when settings are loaded.
    #[serde(rename = "pathTemplate", default, skip_serializing)]
    pub legacy_path_template: Option<String>,
    #[serde(default)]
    pub soulseek_username: Option<String>,
    /// Share `music_dir` on Soulseek.
    #[serde(default = "default_true")]
    pub share_music_dir: bool,
    /// Extra folders to share on Soulseek.
    #[serde(default)]
    pub shared_dirs: Vec<PathBuf>,
    /// Formats that are acceptable to download.
    #[serde(default = "default_formats")]
    pub formats: Vec<AudioFormat>,
    /// Rank 24-bit / high sample rate files above CD quality.
    #[serde(default = "default_true")]
    pub prefer_hi_res: bool,
    /// Download the best lossy copy of tracks with no lossless one.
    #[serde(default = "default_true")]
    pub lossy_fallback: bool,
    /// Browser yt-dlp takes YouTube cookies from, so Premium accounts get
    /// 256 kbps audio. None downloads without signing in.
    #[serde(default)]
    pub youtube_cookies_browser: Option<String>,
    /// Analyse tracks (BPM, beat grid, key, waveforms) as they join the collection.
    #[serde(default = "default_true")]
    pub analyse_new_tracks: bool,
    /// Let a beat grid change tempo part-way through a track, instead of
    /// fitting one tempo to the whole of it as rekordbox's normal analysis does.
    #[serde(default)]
    pub follow_tempo_changes: bool,
}

impl Settings {
    pub fn new(music_dir: PathBuf) -> Self {
        Self {
            music_dir,
            paths: PathTemplates::default(),
            legacy_path_template: None,
            soulseek_username: None,
            share_music_dir: true,
            shared_dirs: Vec::new(),
            formats: default_formats(),
            prefer_hi_res: true,
            lossy_fallback: true,
            youtube_cookies_browser: None,
            analyse_new_tracks: true,
            follow_tempo_changes: false,
        }
    }

    /// Folders slskd should share, in order, skipping any that no longer exist.
    pub fn shares(&self) -> Vec<PathBuf> {
        let music = self.share_music_dir.then(|| self.music_dir.clone());
        music
            .into_iter()
            .chain(self.shared_dirs.iter().cloned())
            .filter(|dir| dir.is_dir())
            .collect()
    }

    /// Whether changing from `self` to `other` requires restarting slskd.
    pub fn daemon_config_differs(&self, other: &Settings) -> bool {
        self.soulseek_username != other.soulseek_username || self.shares() != other.shares()
    }
}

pub struct SettingsState(pub Mutex<Settings>);

impl SettingsState {
    pub fn snapshot(&self) -> Settings {
        self.0.lock().expect("settings lock poisoned").clone()
    }
}

fn default_formats() -> Vec<AudioFormat> {
    AudioFormat::ALL.to_vec()
}

fn default_true() -> bool {
    true
}
