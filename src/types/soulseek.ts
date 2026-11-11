/** Soulseek connection state. Mirrors `SoulseekStatus` in Rust. */
export type SoulseekStatus =
  | { state: 'needsCredentials' }
  | { state: 'connecting'; detail: string | null }
  | { state: 'online'; username: string }
  | { state: 'stopped' }
  | { state: 'error'; message: string };
