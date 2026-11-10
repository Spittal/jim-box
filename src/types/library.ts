import type { TrackMetadata, TrackRequest } from './track';

export interface ExistingTags {
  title: string | null;
  artist: string | null;
  album: string | null;
  albumArtist: string | null;
  trackNumber: number | null;
  discNumber: number | null;
  date: string | null;
  isrc: string | null;
  mbRecordingId: string | null;
  mbReleaseId: string | null;
}

export type Confidence = 'high' | 'medium';

export type ItemStatus =
  | 'pending'
  | 'organized'
  | 'matched'
  | 'unmatched'
  | 'applying'
  | 'applied'
  | 'failed';

/** The release a folder was matched to as a whole. */
export interface AlbumProposal {
  releaseId: string;
  title: string;
  artist: string;
  date: string | null;
  /** Files from the folder matched to the release's tracks. */
  matched: number;
  /** Tracks on the release. */
  tracks: number;
  /** 0 is a perfect match. */
  distance: number;
}

export interface Proposal {
  metadata: TrackMetadata;
  confidence: Confidence;
  targetPath: string;
  request: TrackRequest | null;
  /** Set when the file was matched as part of its album. */
  album: AlbumProposal | null;
}

export interface LibraryItem {
  id: string;
  /** The collection track this file belongs to. */
  trackId: string;
  path: string;
  relativePath: string;
  format: string;
  lossless: boolean;
  durationSecs: number | null;
  bitDepth: number | null;
  sampleRate: number | null;
  /** kbps */
  bitrate: number | null;
  /** Bytes. */
  size: number;
  /** Last modified, in ms since the epoch. */
  modified: number | null;
  tags: ExistingTags;
  proposal: Proposal | null;
  status: ItemStatus;
  error: string | null;
}

export type LibraryPhase = 'idle' | 'scanning' | 'matching' | 'applying' | 'moving';

export interface LibraryProgress {
  phase: LibraryPhase;
  done: number;
  total: number;
}

export interface LibrarySnapshot {
  items: LibraryItem[];
  progress: LibraryProgress;
}

export interface RelocateReport {
  moved: number;
  failed: string[];
}
