/** A volume an export could be written to; see `Device` in src-tauri/src/usb.rs. */
export interface UsbDevice {
  name: string;
  path: string;
  /** 0 when it can't be told. */
  totalBytes: number;
  freeBytes: number;
  fileSystem: string;
  /** What it already holds for players, if anything. */
  holds: { tracks: number; playlists: number; ours: boolean } | null;
  /** The library's playlists it was last written with, when written from this library. */
  playlistIds: string[];
}

/** The format files older players can't read are converted to; null copies every file as it is. */
export type UsbConvert = 'aiff' | 'wav' | 'mp3' | null;

/** What an export would do to a stick, worked out without writing to it. */
export interface UsbEstimate {
  tracks: number;
  tracksNew: number;
  tracksChanged: number;
  tracksKept: number;
  tracksRemoved: number;
  /** Chosen tracks whose file can't be found; they'd be left out. */
  tracksMissing: number;
  copyBytes: number;
  /** The room the export needs while it's written. */
  neededBytes: number;
  freeBytes: number | null;
  approximate: boolean;
}

export type UsbStage =
  | 'preparing'
  | 'checking'
  | 'copying'
  | 'converting'
  | 'database'
  | 'verifying'
  | 'publishing'
  | 'ejecting';

/** Where an export has got to. */
export interface UsbProgress {
  path: string;
  stage: UsbStage;
  done: number;
  total: number;
  title: string;
  fileDone: number;
  fileTotal: number;
  canCancel: boolean;
}

/** How an export went. */
export interface UsbReport {
  path: string;
  /** The stick already held exactly this, and nothing was written. */
  upToDate: boolean;
  tracks: number;
  playlists: number;
  tracksAdded: number;
  tracksUpdated: number;
  removed: number;
  converted: number;
  bytesCopied: number;
  /** "title: reason" for tracks left out. */
  leftOut: string[];
  ejected: boolean;
  ejectError: string | null;
}
