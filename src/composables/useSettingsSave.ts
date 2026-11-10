import { ref } from 'vue';
import { useSettingsStore } from '@/stores/settings';
import type { Settings } from '@/types/settings';

/** Saves a settings change and exposes the backend's error message, if any. */
export function useSettingsSave() {
  const store = useSettingsStore();
  const error = ref<string | null>(null);

  async function save(patch: Partial<Settings>): Promise<boolean> {
    error.value = null;
    try {
      await store.update(patch);
      return true;
    } catch (e) {
      error.value = String(e);
      return false;
    }
  }

  return { error, save };
}
