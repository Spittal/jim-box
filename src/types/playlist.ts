import type { Confidence } from './library';
import type { TrackRequest } from './track';

export interface SpotifyTrack {
  title: string;
  /** Lead artist first. */
  artists: string[];
  durationSecs: number;
}

export interface PlaylistEntry {
  track: SpotifyTrack;
  request: TrackRequest | null;
  confidence: Confidence | null;
}

export interface PlaylistMatch {
  id: string;
  name: string;
  coverUrl: string | null;
  /** Spotify's embed only lists the first 100 tracks. */
  truncated: boolean;
  entries: PlaylistEntry[];
  /** When the track list was last read from Spotify (ms). */
  loadedAt: number;
}

/** A cached playlist, for the recent playlists list. */
export interface PlaylistSummary {
  id: string;
  name: string;
  coverUrl: string | null;
  tracks: number;
  matched: number;
  loadedAt: number;
}

export interface PlaylistProgress {
  done: number;
  total: number;
}

/** What copying a playlist into rekordbox came to. */
export interface PlaylistSync {
  playlistId: string;
  name: string;
  /** Tracks that are in the collection, and so in the rekordbox playlist. */
  present: number;
  /** Tracks that aren't in the collection yet. */
  missing: number;
}
