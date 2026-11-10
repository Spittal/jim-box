import type { AudioFormat } from './settings';
import type { TrackMetadata, TrackRequest } from './track';

export type JobStatus =
  | 'queued'
  | 'identifying'
  | 'searching'
  /** No peer answered; searching again at `retryAt`. */
  | 'waiting'
  | 'downloading'
  | 'tagging'
  | 'completed'
  | 'notFound'
  | 'failed'
  | 'cancelled';

export interface Candidate {
  username: string;
  filename: string;
  size: number;
  format: AudioFormat;
  bitDepth: number | null;
  sampleRate: number | null;
  /** Kilobits per second, as the peer reports it. */
  bitRate: number | null;
  lengthSecs: number | null;
  hasFreeUploadSlot: boolean;
  queueLength: number;
  /** Bytes per second. */
  uploadSpeed: number;
  score: number;
  failed: boolean;
  /** Why the file failed a check; such files are only downloaded by hand. */
  rejected: string | null;
}

/** Where a job looks for the track: YouTube Music only when picked by hand. */
export type JobSource = 'soulseek' | 'youtube';

/** The YouTube Music upload a job downloaded, or is downloading. Lossy AAC. */
export interface YoutubeSource {
  videoId: string;
  title: string;
  channel: string | null;
  url: string;
  lengthSecs: number | null;
  /** kbps once downloaded: about 256 with Premium cookies, else 128. */
  bitRate: number | null;
}

/** One Soulseek search a job ran. */
export interface SearchAttempt {
  query: string;
  /** Peers that responded. */
  peers: number;
  /** Files that passed format, title, version and length checks. */
  usable: number;
  /** Lossless files with the right title that failed a check. */
  rejected: number;
}

export interface TransferProgress {
  bytesTransferred: number;
  size: number;
  /** Bytes per second. */
  speed: number;
  placeInQueue: number | null;
}

export interface Job {
  id: string;
  request: TrackRequest;
  metadata: TrackMetadata | null;
  status: JobStatus;
  candidates: Candidate[];
  searches: SearchAttempt[];
  activeCandidate: number | null;
  transferId: string | null;
  progress: TransferProgress | null;
  /** When a waiting job searches again, in ms since the epoch. */
  retryAt: number | null;
  outputPath: string | null;
  replaces: string | null;
  /** Completed without downloading because `outputPath` already had this track. */
  alreadyInLibrary: boolean;
  /** The rekordbox playlist (its id) the track joins once it's in the collection. */
  playlistId: string | null;
  error: string | null;
  source: JobSource;
  /** Set while and after downloading from YouTube Music. */
  youtube: YoutubeSource | null;
  createdAt: number;
  updatedAt: number;
}
