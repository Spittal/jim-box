import type { Confidence } from './library';
import type { TrackRequest } from './track';

export interface SpotifyTrack {
  title: string;
  /** Lead artist first. */
  artists: string[];
  durationSecs: number;
}

/** A track of a Spotify playlist that isn't in the collection yet. */
export interface MissingEntry {
  /** Its place in the Spotify playlist. */
  index: number;
  track: SpotifyTrack;
  /** The search result it was matched to; null when no confident match was found. */
  request: TrackRequest | null;
  confidence: Confidence | null;
}

/** A rekordbox playlist that came from Spotify, and what it still lacks. */
export interface LinkedPlaylist {
  spotifyId: string;
  /** The rekordbox playlist. */
  playlistId: string;
  name: string;
  /** Spotify only lists a playlist's first 100 tracks without signing in. */
  truncated: boolean;
  /** When the track list was last read from Spotify (ms). */
  loadedAt: number;
  /** Tracks on the Spotify playlist. */
  total: number;
  /** Those that aren't in the collection yet, in the playlist's order. */
  missing: MissingEntry[];
}

/** What importing a Spotify playlist once did. */
export interface Imported {
  /** The Spotify playlist's name. */
  name: string;
  /** Spotify only lists a playlist's first 100 tracks without signing in. */
  truncated: boolean;
  /** Tracks on the Spotify playlist. */
  total: number;
  /** Collection tracks put in the rekordbox playlist. */
  added: number;
  /** Those the rekordbox playlist had already. */
  present: number;
  /** Sure matches now being fetched; they land in the playlist. */
  fetching: number;
  /** The rest: no sure match, or fetched before. */
  skipped: number;
}

/** How a rekordbox playlist follows its Spotify playlist. */
export interface SyncSettings {
  /** Fetch tracks added on Spotify whose match is sure. */
  autoFetch: boolean;
  /**
   * 1-to-1: keep the rekordbox playlist to exactly the Spotify playlist's
   * tracks, taking out whatever isn't on Spotify.
   */
  mirror: boolean;
}

/** A rekordbox playlist's link to Spotify. */
export interface Followed {
  spotifyId: string;
  /** The Spotify playlist's name. */
  name: string;
  settings: SyncSettings;
}

export interface PlaylistProgress {
  done: number;
  total: number;
}
