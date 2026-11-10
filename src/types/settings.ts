/** Lossless formats, which can be chosen in settings. */
export type LosslessFormat = 'flac' | 'alac' | 'aiff' | 'wav';
/** Lossy formats, downloaded only as a fallback. */
export type LossyFormat = 'mp3' | 'aac' | 'vorbis' | 'opus';
export type AudioFormat = LosslessFormat | LossyFormat;

const LOSSLESS: AudioFormat[] = ['flac', 'alac', 'aiff', 'wav'];

export function isLossless(format: AudioFormat): boolean {
  return LOSSLESS.includes(format);
}

/** Beets-style `paths`: templates for album tracks, compilations and tracks with no album. */
export interface PathTemplates {
  default: string;
  comp: string;
  singleton: string;
}

export interface Settings {
  musicDir: string;
  paths: PathTemplates;
  soulseekUsername: string | null;
  shareMusicDir: boolean;
  sharedDirs: string[];
  formats: LosslessFormat[];
  preferHiRes: boolean;
  /** Download the best lossy copy of tracks with no lossless one. */
  lossyFallback: boolean;
  /** Browser yt-dlp takes YouTube cookies from, for Premium's 256 kbps audio. */
  youtubeCookiesBrowser: CookieBrowser | null;
  /** Analyse tracks (BPM, beat grid, key, waveforms) as they join the collection. */
  analyseNewTracks: boolean;
  /** Let a beat grid change tempo part-way through a track. */
  followTempoChanges: boolean;
}

export type CookieBrowser = 'safari' | 'chrome' | 'edge' | 'firefox' | 'brave' | 'vivaldi';

/** The yt-dlp jim-box uses for YouTube Music downloads. */
export interface YtDlpInfo {
  path: string;
  version: string;
}
