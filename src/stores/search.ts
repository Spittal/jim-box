import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { TrackRequest } from '@/types/track';
import { isPlaylistLink, usePlaylistStore } from './playlist';

export const useSearchStore = defineStore('search', () => {
  const query = ref('');
  const results = ref<TrackRequest[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  /** The query the current results belong to. */
  const searchedFor = ref('');

  /** Searches tracks, or loads a playlist when given a Spotify playlist link. */
  async function search() {
    const q = query.value.trim();
    if (!q) return;
    const playlistStore = usePlaylistStore();
    if (isPlaylistLink(q)) {
      results.value = [];
      searchedFor.value = '';
      error.value = null;
      await playlistStore.load(q);
      return;
    }
    playlistStore.clear();
    loading.value = true;
    error.value = null;
    try {
      results.value = await invoke<TrackRequest[]>('search_tracks', { query: q });
      searchedFor.value = q;
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Searches tracks without touching the page's results, e.g. to re-match one playlist track. */
  const find = (q: string) => invoke<TrackRequest[]>('search_tracks', { query: q });

  return { query, results, loading, error, searchedFor, search, find };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useSearchStore, import.meta.hot));
