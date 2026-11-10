import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { TrackRequest } from '@/types/track';

/** Looking for tracks online: Deezer's results, then MusicBrainz's. */
export const useSearchStore = defineStore('search', () => {
  /** Whether the results panel is showing. */
  const open = ref(false);
  const results = ref<TrackRequest[]>([]);
  /** For each result, the collection track it already is, if any. */
  const owned = ref<(string | null)[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  /** The text the current results belong to. */
  const searchedFor = ref('');
  /** Identifies the latest search, so an earlier one's late answer is dropped. */
  let latest = 0;

  const find = (query: string) => invoke<TrackRequest[]>('search_tracks', { query });

  async function run(text: string) {
    const query = text.trim();
    if (!query) return;
    const mine = ++latest;
    open.value = true;
    loading.value = true;
    error.value = null;
    try {
      const found = await find(query);
      if (mine !== latest) return;
      results.value = found;
      owned.value = [];
      searchedFor.value = query;
      await refreshOwned();
    } catch (e) {
      if (mine === latest) error.value = String(e);
    } finally {
      if (mine === latest) loading.value = false;
    }
  }

  /** Looks the results up in the collection again, as after it changes. */
  async function refreshOwned() {
    const asked = results.value;
    if (!asked.length) return;
    try {
      const ids = await invoke<(string | null)[]>('find_in_collection', {
        tracks: asked.map((t) => ({ title: t.title, artist: t.artist })),
      });
      if (results.value === asked) owned.value = ids;
    } catch {
      // The collection isn't open: nothing is marked as owned.
    }
  }

  /** Shows a message in the panel in place of results. */
  function fail(message: string) {
    latest++;
    loading.value = false;
    open.value = true;
    error.value = message;
  }

  function close() {
    open.value = false;
    error.value = null;
  }

  return { open, results, owned, loading, error, searchedFor, find, run, refreshOwned, fail, close };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useSearchStore, import.meta.hot));
