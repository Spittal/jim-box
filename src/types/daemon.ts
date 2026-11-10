/** slskd / Soulseek connection state. Mirrors `DaemonStatus` in Rust. */
export type DaemonStatus =
  | { state: 'needsCredentials' }
  | { state: 'starting' }
  | { state: 'connecting'; detail: string | null }
  | { state: 'online'; username: string }
  | { state: 'stopped' }
  | { state: 'error'; message: string };
