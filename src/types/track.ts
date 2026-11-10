/** A track picked from search results: from Deezer, or MusicBrainz for tracks Deezer lacks. */
export interface TrackRequest {
  deezerId: number | null;
  mbRecordingId: string | null;
  title: string;
  artist: string;
  album: string;
  isrc: string | null;
  durationSecs: number;
  trackNumber: number | null;
  discNumber: number | null;
  releaseDate: string | null;
  coverUrl: string | null;
  coverXlUrl: string | null;
  /** Deezer's 30-second clip. The link stops working after about 15 minutes. */
  previewUrl?: string | null;
}

/** Canonical metadata written to tags. Mirrors `TrackMetadata` in Rust. */
export interface TrackMetadata {
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  primaryAlbumArtist: string | null;
  /** A Various Artists compilation (beets' `comp`). */
  compilation: boolean;
  trackNumber: number | null;
  trackTotal: number | null;
  discNumber: number | null;
  date: string | null;
  isrc: string | null;
  durationMs: number | null;
  mbRecordingId: string | null;
  mbReleaseId: string | null;
  mbReleaseGroupId: string | null;
  mbArtistId: string | null;
  mbAlbumArtistId: string | null;
  coverArtUrls: string[];
}

/** Identifies a search result across both sources. */
export function trackKey(track: TrackRequest): string {
  return track.deezerId !== null ? `dz:${track.deezerId}` : `mb:${track.mbRecordingId}`;
}
