import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { LinkedPlaylist, PlaylistProgress } from '@/types/playlist';
import type { TrackRequest } from '@/types/track';

const PLAYLIST_LINK = /open\.spotify\.com\/(?:[\w-]+\/)?playlist\/[A-Za-z0-9]{22}|^spotify:playlist:[A-Za-z0-9]{22}$/;

export function isPlaylistLink(input: string): boolean {
  return PLAYLIST_LINK.test(input.trim());
}

/** Spotify playlists brought into rekordbox as playlists. */
export const usePlaylistStore = defineStore('playlist', () => {
  /** The Spotify side of the rekordbox playlist being shown, if it came from one. */
  const linked = ref<LinkedPlaylist | null>(null);
  const importing = ref(false);
  /** Tracks matched so far while importing. */
  const progress = ref<PlaylistProgress | null>(null);
  /** Identifies the playlist being shown, so a late answer for another is dropped. */
  let shown: string | null = null;

  /**
   * Brings a Spotify playlist into rekordbox: a playlist with the tracks the
   * collection has. Tracks matched before keep their match. Throws on failure.
   */
  async function importLink(url: string): Promise<LinkedPlaylist> {
    importing.value = true;
    progress.value = null;
    try {
      const result = await invoke<LinkedPlaylist>('import_spotify_playlist', { url });
      if (shown === result.playlistId) linked.value = result;
      return result;
    } finally {
      importing.value = false;
      progress.value = null;
    }
  }

  /** Reads the open playlist from Spotify again, to pick up added tracks. */
  async function refresh() {
    if (linked.value) await importLink(`https://open.spotify.com/playlist/${linked.value.spotifyId}`);
  }

  /** Shows the Spotify side of a rekordbox playlist, as the collection now is. */
  async function show(playlistId: string | null) {
    if (shown !== playlistId) linked.value = null;
    shown = playlistId;
    if (!playlistId) return;
    try {
      const result = await invoke<LinkedPlaylist | null>('linked_playlist', { playlistId });
      if (shown === playlistId) linked.value = result;
    } catch {
      // The collection isn't open; the next change asks again.
    }
  }

  /** Replaces a missing track's match with one the user picked, and saves it. */
  async function setMatch(index: number, request: TrackRequest) {
    const current = linked.value;
    if (!current) return;
    await invoke('set_playlist_match', { id: current.spotifyId, index, request });
    const entry = current.missing.find((m) => m.index === index);
    if (entry) {
      entry.request = request;
      entry.confidence = 'high';
    }
  }

  return { linked, importing, progress, importLink, refresh, show, setMatch };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(usePlaylistStore, import.meta.hot));
