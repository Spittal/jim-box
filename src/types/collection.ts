export interface CollectionStatus {
  masterDb: string;
  tracks: number;
  playlists: number;
  /** Changes are refused while rekordbox (or its agent) is running. */
  rekordboxRunning: boolean;
}

export type PlaylistKind = 'playlist' | 'folder' | 'smart';

/** A row of the playlist tree, in display order. */
export interface PlaylistNode {
  id: string;
  name: string;
  parentId: string | null;
  depth: number;
  kind: PlaylistKind;
  /** Tracks in a playlist; null for folders and intelligent playlists. */
  count: number | null;
}

export type TrackSource = { kind: 'collection' } | { kind: 'playlist'; id: string };

export type TrackSort =
  | 'trackNo'
  | 'title'
  | 'artist'
  | 'album'
  | 'genre'
  | 'bpm'
  | 'key'
  | 'duration'
  | 'rating'
  | 'dateAdded'
  | 'path'
  | 'playlists';

export interface TrackQuery {
  source: TrackSource;
  sort: TrackSort;
  descending: boolean;
  query: string;
}

export interface CollectionTrack {
  id: string;
  /** Position in the playlist, or in the list. */
  position: number;
  title: string;
  artist: string;
  album: string;
  genre: string;
  key: string;
  bpm: number | null;
  durationSecs: number;
  /** Stars, 0 to 5. */
  rating: number;
  /** rekordbox's colour, 1 to 8; 0 for none. */
  color: number;
  dateAdded: string;
  format: string;
  /** kbps */
  bitrate: number;
  path: string;
  comment: string;
  /** The playlists it's in, by name. Intelligent playlists aren't counted. */
  playlists: string[];
  analysed: boolean;
}

export interface TrackPage {
  /** Tracks in the whole list, not just this page. */
  total: number;
  offset: number;
  rows: CollectionTrack[];
}

/** A text field of a track that can be edited. */
export type TrackField = 'title' | 'artist' | 'album' | 'genre' | 'label' | 'year';

/** One change to the library; see `Edit` in src-tauri/src/rekordbox/edit.rs. */
export type CollectionEdit =
  | { kind: 'createPlaylist'; name: string; parentId: string | null; folder: boolean }
  | { kind: 'renamePlaylist'; id: string; name: string }
  | { kind: 'deletePlaylist'; id: string }
  | { kind: 'movePlaylist'; id: string; parentId: string | null; index: number | null }
  | { kind: 'addTracks'; playlistId: string; trackIds: string[] }
  | { kind: 'removeTracks'; playlistId: string; trackIds: string[] }
  | { kind: 'moveTracks'; playlistId: string; trackIds: string[]; before: string | null }
  | { kind: 'deleteTracks'; trackIds: string[] }
  | { kind: 'setRating'; trackId: string; stars: number }
  | { kind: 'setColor'; trackId: string; color: number }
  | { kind: 'setComment'; trackId: string; comment: string }
  | { kind: 'setField'; trackId: string; field: TrackField; value: string }
  | { kind: 'addCue'; trackId: string; slot: number | null; positionMs: number; endMs: number | null }
  | { kind: 'deleteCue'; cueId: string }
  | { kind: 'nameCue'; cueId: string; comment: string }
  /** A beat grid change: to all of it, or from the beat nearest `fromMs` on. */
  | { kind: 'grid'; trackId: string; change: GridChange; fromMs: number | null }
  | { kind: 'lockAnalysis'; trackId: string; locked: boolean };

/** A change to a track's beat grid. */
export type GridChange =
  | { kind: 'nudge'; ms: number }
  /** Slides the grid so its nearest beat is at `timeMs`. */
  | { kind: 'align'; timeMs: number }
  /** Slides the grid so its nearest beat is at `timeMs`, and starts a bar there. */
  | { kind: 'downbeat'; timeMs: number }
  | { kind: 'double' }
  | { kind: 'halve' }
  | { kind: 'tempo'; bpm: number }
  /** Widens or narrows the beats' spacing: moves the beat nearest `timeMs` by `byMs`. */
  | { kind: 'stretch'; byMs: number; timeMs: number }
  /** A tapped tempo, with a bar starting where the first tap was. */
  | { kind: 'tap'; bpm: number; anchorMs: number }
  /** Puts back a grid as it was read, for undo. */
  | { kind: 'restore'; beats: Beat[] };

/** A beat: [milliseconds, place in the bar 1 to 4, tempo in hundredths of a BPM]. */
export type Beat = [number, number, number];

export interface Cue {
  id: string;
  /** The hot cue pad, 0 for A; null for a memory cue. */
  slot: number | null;
  positionMs: number;
  /** Where a loop ends; null for a plain cue. */
  endMs: number | null;
  comment: string;
  /** The colour rekordbox draws a hot cue in, as #rrggbb; null for a memory cue. */
  color: string | null;
}

/** A track as the player needs it. */
export interface Preparation {
  trackId: string;
  durationMs: number;
  /** Empty before analysis. */
  beats: Beat[];
  bpm: number | null;
  cues: Cue[];
  /** rekordbox's analysis lock: the grid can't be changed. */
  locked: boolean;
}

export interface EditOutcome {
  status: CollectionStatus;
  playlists: PlaylistNode[];
  /** The playlist or folder a `createPlaylist` made. */
  createdId: string | null;
}

/** Files waiting to join the collection, which rekordbox being open holds up. */
export interface Waiting {
  count: number;
  /** Why the last attempt to add them didn't work. */
  error: string | null;
}

export interface CollectionState {
  status: CollectionStatus;
  playlists: PlaylistNode[];
  waiting: Waiting;
}

export interface ImportReport {
  added: number;
  /** Files that were in the collection already. */
  existing: number;
  /** "file: reason" for files that couldn't be read. */
  skipped: string[];
  trackIds: string[];
}

export interface AnalysisFailure {
  trackId: string;
  title: string;
  error: string;
}

/** The analysis queue's current batch: from the first track queued until it's empty. */
export interface AnalysisProgress {
  active: boolean;
  total: number;
  done: number;
  /** Ids of the tracks being analysed right now. */
  running: string[];
  failed: AnalysisFailure[];
  /** Why the batch ended early, when it did. */
  stopped: string | null;
}
