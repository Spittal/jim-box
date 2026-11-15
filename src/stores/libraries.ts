import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { LibraryStatus } from '@/types/libraries';

/** The rekordbox library Jim Box manages: the one in rekordbox's default folder on this computer. */
export const useLibrariesStore = defineStore('libraries', () => {
  const status = ref<LibraryStatus | null>(null);

  async function load() {
    status.value = await invoke<LibraryStatus>('library_status');
  }

  return { status, load };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useLibrariesStore, import.meta.hot));
