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

export interface PlaylistProgress {
  done: number;
  total: number;
}
