import { defineStore } from 'pinia';
import { ref } from 'vue';

export type SettingsTab = 'soulseek' | 'library' | 'sharing' | 'formats' | 'youtube';

/** What is open over the collection. */
export const useUiStore = defineStore('ui', () => {
  /** The settings tab being shown; null when settings are closed. */
  const settings = ref<SettingsTab | null>(null);

  function toggleSettings(tab: SettingsTab = 'library') {
    settings.value = settings.value ? null : tab;
  }

  return { settings, toggleSettings };
});
