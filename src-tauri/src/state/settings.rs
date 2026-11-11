use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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

/// How musical keys are shown, whichever way a library stores them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyNotation {
    /// Musical names: `Am`, `F#`.
    #[default]
    Standard,
    /// Camelot wheel codes: `8A`, `2B`.
    Camelot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The primary library folder: finished downloads are moved here.
    pub music_dir: PathBuf,
    /// Other folders the library is kept in. Their files are tidied in
    /// place and shared like the primary's; new tracks don't go to them.
    #[serde(default)]
    pub library_dirs: Vec<PathBuf>,
    /// How organized files are laid out in a library folder.
    #[serde(default)]
    pub paths: PathTemplates,
    /// The single template of older versions, in `{field}` syntax; migrated
    /// into `paths.default` when settings are loaded.
    #[serde(rename = "pathTemplate", default, skip_serializing)]
    pub legacy_path_template: Option<String>,
    #[serde(default)]
    pub soulseek_username: Option<String>,
    /// Share the library folders on Soulseek.
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
    /// Let a beat grid follow the tempo where both beat detectors find it
    /// moving (a drummer who drifts, a DJ edit that changes pace) instead
    /// of fitting one tempo to the whole track as rekordbox's normal
    /// analysis does.
    #[serde(default = "default_true")]
    pub dynamic_grids: bool,
    /// How musical keys are shown.
    #[serde(default)]
    pub key_notation: KeyNotation,
}

impl Settings {
    pub fn new(music_dir: PathBuf) -> Self {
        Self {
            music_dir,
            library_dirs: Vec::new(),
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
            dynamic_grids: true,
            key_notation: KeyNotation::default(),
        }
    }

    /// Every library folder, the primary first.
    pub fn library_roots(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.music_dir.as_path()).chain(self.library_dirs.iter().map(PathBuf::as_path))
    }

    /// The library folder `path` is in, if it's in one.
    pub fn library_root_of(&self, path: &Path) -> Option<&Path> {
        self.library_roots().find(|root| path.starts_with(root))
    }

    /// The library folder `path` is in, or the primary one for a file
    /// that's in none: what its path is shown relative to.
    pub fn library_root_for(&self, path: &Path) -> &Path {
        self.library_root_of(path).unwrap_or(&self.music_dir)
    }

    /// Folders slskd should share, in order, each once, skipping any that
    /// no longer exist.
    pub fn shares(&self) -> Vec<PathBuf> {
        let library = self.library_roots().filter(|_| self.share_music_dir);
        let mut shares: Vec<PathBuf> = Vec::new();
        for dir in library.chain(self.shared_dirs.iter().map(PathBuf::as_path)) {
            if dir.is_dir() && !shares.iter().any(|shared| shared == dir) {
                shares.push(dir.to_path_buf());
            }
        }
        shares
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_library_folder_is_shared_once() {
        let dir = tempfile::tempdir().unwrap();
        let (primary, second, extra) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("c"));
        [&primary, &second, &extra].iter().for_each(|d| std::fs::create_dir_all(d).unwrap());
        let mut settings = Settings::new(primary.clone());
        settings.library_dirs = vec![second.clone(), dir.path().join("gone")];
        settings.shared_dirs = vec![second.clone(), extra.clone()];
        assert_eq!(settings.shares(), vec![primary.clone(), second.clone(), extra.clone()]);
        settings.share_music_dir = false;
        assert_eq!(settings.shares(), vec![second.clone(), extra]);

        assert_eq!(settings.library_root_of(&second.join("x/y.flac")), Some(second.as_path()));
        assert_eq!(settings.library_root_of(&dir.path().join("ab/y.flac")), None);
        assert_eq!(settings.library_root_for(&dir.path().join("ab/y.flac")), primary.as_path());
    }
}
