import { defineStore } from 'pinia';
import { ref, watch } from 'vue';

export type SettingsTab = 'soulseek' | 'library' | 'sharing' | 'formats' | 'youtube' | 'shortcuts';

/** What is open over the collection. */
export const useUiStore = defineStore('ui', () => {
  /** The settings tab being shown; null when settings are closed. */
  const settings = ref<SettingsTab | null>(null);

  /** Set to bring the Library tab's Consolidate part into view. */
  const consolidate = ref(false);

  /** Whether the Soulseek tab is asking for a sign-in, as finding online needs one. */
  const soulseekPrompt = ref(false);

  /** Opens the Soulseek tab, asking for the account that finding online needs. */
  function promptSoulseek() {
    settings.value = 'soulseek';
    soulseekPrompt.value = true;
  }

  // The request goes with the tab: opened another way later, it only shows the settings.
  watch(settings, (tab) => {
    if (tab !== 'soulseek') soulseekPrompt.value = false;
  });

  function showConsolidate() {
    settings.value = 'library';
    consolidate.value = true;
  }

  function toggleSettings(tab: SettingsTab = 'library') {
    settings.value = settings.value ? null : tab;
  }

  /** Shows the keyboard shortcuts; or, when they're showing, closes them. */
  function toggleShortcuts() {
    settings.value = settings.value === 'shortcuts' ? null : 'shortcuts';
  }

  return { settings, consolidate, soulseekPrompt, toggleSettings, toggleShortcuts, showConsolidate, promptSoulseek };
});
