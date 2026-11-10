import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { DaemonStatus } from '@/types/daemon';

export const useDaemonStore = defineStore('daemon', () => {
  const status = ref<DaemonStatus>({ state: 'starting' });

  const isOnline = computed(() => status.value.state === 'online');
  const needsCredentials = computed(() => status.value.state === 'needsCredentials');

  async function load() {
    status.value = await invoke<DaemonStatus>('get_daemon_status');
  }

  async function restart() {
    await invoke('restart_daemon');
  }

  return { status, isOnline, needsCredentials, load, restart };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useDaemonStore, import.meta.hot));
