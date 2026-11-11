import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { LibraryChoices, Opened } from '@/types/libraries';

/** The rekordbox libraries jim-box manages, one of them open at a time. */
export const useLibrariesStore = defineStore('libraries', () => {
  const choices = ref<LibraryChoices | null>(null);

  /** The open library. */
  const current = computed(
    () => choices.value?.libraries.find((library) => library.id !== null && library.id === choices.value?.open) ?? null,
  );
  const opensByDefault = computed(() => !!current.value && choices.value?.defaultId === current.value.id);

  async function load() {
    choices.value = await invoke<LibraryChoices>('library_choices');
  }

  /**
   * Opens a library, and has it open by default from now on, or has none do.
   * With another library open the app restarts into this one.
   */
  async function open(masterDb: string, makeDefault: boolean): Promise<Opened> {
    const opened = await invoke<Opened>('open_library', { masterDb, makeDefault });
    if (opened === 'open') await load();
    return opened;
  }

  /** Has the open library open by default, or has the app ask at each launch. */
  async function setDefault(on: boolean) {
    choices.value = await invoke<LibraryChoices>('set_default_library', { on });
  }

  /** Drops a library from the list, with what jim-box saved for it. */
  async function forget(id: string) {
    choices.value = await invoke<LibraryChoices>('forget_library', { id });
  }

  return { choices, current, opensByDefault, load, open, setDefault, forget };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useLibrariesStore, import.meta.hot));
