/** A rekordbox library (a `master.db`) jim-box can manage. */
export interface LibraryInfo {
  /** Null for one found on this machine that hasn't been opened yet. */
  id: string | null;
  masterDb: string;
  name: string;
  /** False when the file isn't there, as when its drive isn't connected. */
  available: boolean;
  lastOpened: number | null;
}

export interface LibraryChoices {
  /** The library that is open, if one is. */
  open: string | null;
  /** The library that opens without asking. */
  defaultId: string | null;
  /** Those opened before, the latest first, then others found on this machine. */
  libraries: LibraryInfo[];
}

/** `restarting`: another library was open, and the app is restarting into the chosen one. */
export type Opened = 'open' | 'restarting';
