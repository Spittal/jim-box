import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { SoulseekStatus } from '@/types/soulseek';

export const useSoulseekStore = defineStore('soulseek', () => {
  const status = ref<SoulseekStatus>({ state: 'connecting', detail: null });

  const isOnline = computed(() => status.value.state === 'online');
  const needsCredentials = computed(() => status.value.state === 'needsCredentials');

  async function load() {
    status.value = await invoke<SoulseekStatus>('get_soulseek_status');
  }

  async function reconnect() {
    await invoke('reconnect_soulseek');
  }

  return { status, isOnline, needsCredentials, load, reconnect };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useSoulseekStore, import.meta.hot));
