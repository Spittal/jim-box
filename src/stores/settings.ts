import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Settings } from '@/types/settings';

export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<Settings | null>(null);

  async function load() {
    settings.value = await invoke<Settings>('get_settings');
  }

  /** Saves a change; throws the backend's validation message on failure. */
  async function update(patch: Partial<Settings>) {
    if (!settings.value) return;
    settings.value = await invoke<Settings>('update_settings', {
      settings: { ...settings.value, ...patch },
    });
  }

  async function setCredentials(username: string, password: string) {
    await invoke('set_soulseek_credentials', { username, password });
    await load();
  }

  return { settings, load, update, setCredentials };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useSettingsStore, import.meta.hot));
