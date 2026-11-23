import type { CollectionStatus, PlaylistNode } from './collection';
import type { UsbReport } from './usb';

/** Whether the rekordbox library is open, and where it is; see `LibraryStatus` in src-tauri/src/libraries.rs. */
export interface LibraryStatus {
  open: boolean;
  /** The open library's `master.db`, else where rekordbox would keep one; null when that can't be told. */
  masterDb: string | null;
}

/** What an opened library is: another `master.db`, or a USB stick's export. */
export type OtherKind = 'library' | 'stick';

/** The changes waiting to be written to a stick, in a few numbers; see `StagedSummary` in src-tauri/src/rekordbox/stick.rs. */
export interface StagedSummary {
  changes: number;
  playlistsMade: number;
  renamed: number;
  deleted: number;
  /** Playlists of the library to write. */
  exported: number;
  added: number;
  removed: number;
  /** The library's tracks to copy onto the stick, each once. */
  tracksToCopy: number;
}

/** A library opened beside the master; see `OtherInfo` in src-tauri/src/others.rs. Its ids all start `<id>~`. */
export interface OtherLibrary {
  id: string;
  name: string;
  /** A stick's root, or the library's master.db. */
  path: string;
  kind: OtherKind;
  status: CollectionStatus;
  playlists: PlaylistNode[];
  /** A stick's changes waiting to be written; null for a library. */
  staged: StagedSummary | null;
}

/** A drive plugged in with a library on it that isn't open. */
export interface PluggedLibrary {
  name: string;
  path: string;
  kind: OtherKind;
}

/** What taking tracks or playlists from one library to another did; see `Moved` in src-tauri/src/others.rs. */
export interface Moved {
  added: number;
  updated: number;
  /** "title: reason" for each that couldn't be brought in, or only partly. */
  failed: string[];
  trackIds: string[];
  playlistId: string | null;
  /** The library they went to; null for the master. */
  library: string | null;
  /** Kept to write to a stick later, rather than done. */
  staged: boolean;
}

/** What writing a stick's changes did; see `Applied` in src-tauri/src/others.rs. */
export interface Applied {
  export: UsbReport | null;
  edits: number;
  problems: string[];
  ejected: boolean;
  ejectError: string | null;
}
