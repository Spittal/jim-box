/** Whether the rekordbox library is open, and where it is; see `LibraryStatus` in src-tauri/src/libraries.rs. */
export interface LibraryStatus {
  open: boolean;
  /** The open library's `master.db`, else where rekordbox would keep one; null when that can't be told. */
  masterDb: string | null;
}
