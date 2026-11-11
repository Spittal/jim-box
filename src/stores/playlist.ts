import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Followed, LinkedPlaylist, PlaylistProgress, SyncSettings } from '@/types/playlist';
import type { TrackRequest } from '@/types/track';

const PLAYLIST_LINK = /open\.spotify\.com\/(?:[\w-]+\/)?playlist\/[A-Za-z0-9]{22}|^spotify:playlist:[A-Za-z0-9]{22}$/;

export function isPlaylistLink(input: string): boolean {
  return PLAYLIST_LINK.test(input.trim());
}

/** rekordbox playlists that follow a Spotify playlist. */
export const usePlaylistStore = defineStore('playlist', () => {
  /** The Spotify side of the rekordbox playlist being shown, if it came from one. */
  const linked = ref<LinkedPlaylist | null>(null);
  /** The rekordbox playlists that follow a Spotify playlist, by id. */
  const watched = ref(new Map<string, Followed>());
  const importing = ref(false);
  /** Tracks matched so far while importing. */
  const progress = ref<PlaylistProgress | null>(null);
  /** Identifies the playlist being shown, so a late answer for another is dropped. */
  let shown: string | null = null;

  async function loadWatched() {
    try {
      watched.value = new Map(Object.entries(await invoke<Record<string, Followed>>('followed_playlists')));
    } catch {
      // No library is open yet.
    }
  }

  /**
   * Makes a rekordbox playlist follow a Spotify playlist: it gets the
   * tracks the collection has, the rest are fetched, and tracks added on
   * Spotify later are too. Throws on failure.
   */
  async function link(url: string, playlistId: string): Promise<LinkedPlaylist> {
    importing.value = true;
    progress.value = null;
    try {
      const result = await invoke<LinkedPlaylist>('link_spotify_playlist', { url, playlistId });
      if (shown === result.playlistId) linked.value = result;
      loadWatched();
      return result;
    } finally {
      importing.value = false;
      progress.value = null;
    }
  }

  /** Reads the open playlist from Spotify again now, and looks again for tracks with no match. */
  async function refresh() {
    const current = linked.value;
    if (current) await link(`https://open.spotify.com/playlist/${current.spotifyId}`, current.playlistId);
  }

  /** Stops a playlist following Spotify. The playlist stays as it is. */
  async function unlink(playlistId: string) {
    await invoke('unlink_playlist', { playlistId });
    if (linked.value?.playlistId === playlistId) linked.value = null;
    await loadWatched();
  }

  /** Changes how a playlist follows Spotify. */
  async function setSettings(playlistId: string, settings: SyncSettings) {
    await invoke('set_playlist_settings', { playlistId, settings });
    await loadWatched();
  }

  /** The backend read a followed playlist again: show what it found. */
  function synced(playlistId: string) {
    loadWatched();
    if (shown === playlistId) show(playlistId);
  }

  /** Shows the Spotify side of a rekordbox playlist, as the collection now is. */
  async function show(playlistId: string | null) {
    if (shown !== playlistId) linked.value = null;
    shown = playlistId;
    // Deleting a playlist in rekordbox, or here, stops it being followed.
    loadWatched();
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

  return { linked, watched, importing, progress, link, refresh, unlink, setSettings, synced, show, setMatch };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(usePlaylistStore, import.meta.hot));
