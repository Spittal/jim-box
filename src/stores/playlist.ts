import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { PlaylistMatch, PlaylistProgress, PlaylistSummary, PlaylistSync } from '@/types/playlist';
import type { TrackRequest } from '@/types/track';

const PLAYLIST_LINK = /open\.spotify\.com\/(?:[\w-]+\/)?playlist\/[A-Za-z0-9]{22}|^spotify:playlist:[A-Za-z0-9]{22}$/;

export function isPlaylistLink(input: string): boolean {
  return PLAYLIST_LINK.test(input.trim());
}

export const usePlaylistStore = defineStore('playlist', () => {
  const playlist = ref<PlaylistMatch | null>(null);
  /** Cached playlists, most recent first. */
  const recent = ref<PlaylistSummary[]>([]);
  const loading = ref(false);
  const progress = ref<PlaylistProgress | null>(null);
  const error = ref<string | null>(null);

  async function loadRecent() {
    recent.value = await invoke<PlaylistSummary[]>('list_playlists');
  }

  async function withLoading(fetch: () => Promise<PlaylistMatch>) {
    loading.value = true;
    error.value = null;
    progress.value = null;
    try {
      playlist.value = await fetch();
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Reads a playlist from Spotify; tracks matched before reuse their match. */
  async function load(url: string) {
    playlist.value = null;
    await withLoading(() => invoke<PlaylistMatch>('load_spotify_playlist', { url }));
    await loadRecent();
  }

  /** Opens a cached playlist as last loaded, without contacting Spotify. */
  const open = (id: string) => withLoading(() => invoke<PlaylistMatch>('get_playlist', { id }));

  /** Re-reads the open playlist from Spotify to pick up added tracks. */
  async function refresh() {
    if (!playlist.value) return;
    const url = `https://open.spotify.com/playlist/${playlist.value.id}`;
    await withLoading(() => invoke<PlaylistMatch>('load_spotify_playlist', { url }));
    await loadRecent();
  }

  /** Replaces a track's match with one the user picked, and saves it. */
  async function setMatch(index: number, request: TrackRequest) {
    const current = playlist.value;
    const entry = current?.entries[index];
    if (!current || !entry) return;
    await invoke('set_playlist_match', { id: current.id, index, request });
    entry.request = request;
    entry.confidence = 'high';
  }

  async function remove(id: string) {
    await invoke('remove_playlist', { id });
    await loadRecent();
  }

  /** Copies the open playlist into rekordbox, as far as its tracks are in the collection. */
  const syncToRekordbox = (id: string) => invoke<PlaylistSync>('sync_playlist_to_rekordbox', { id });

  function clear() {
    playlist.value = null;
    error.value = null;
    progress.value = null;
  }

  return {
    playlist,
    recent,
    loading,
    progress,
    error,
    loadRecent,
    load,
    open,
    refresh,
    setMatch,
    remove,
    syncToRekordbox,
    clear,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(usePlaylistStore, import.meta.hot));
