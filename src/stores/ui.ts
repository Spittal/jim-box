import { defineStore } from 'pinia';
import { ref } from 'vue';

export type SettingsTab = 'soulseek' | 'library' | 'sharing' | 'formats' | 'youtube';

/** What is open over the collection. */
export const useUiStore = defineStore('ui', () => {
  /** The settings tab being shown; null when settings are closed. */
  const settings = ref<SettingsTab | null>(null);

  /** Set to bring the Library tab's Consolidate part into view. */
  const consolidate = ref(false);

  function showConsolidate() {
    settings.value = 'library';
    consolidate.value = true;
  }

  function toggleSettings(tab: SettingsTab = 'library') {
    settings.value = settings.value ? null : tab;
  }

  return { settings, consolidate, toggleSettings, showConsolidate };
});
