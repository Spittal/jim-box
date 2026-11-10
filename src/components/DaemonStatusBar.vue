<script setup lang="ts">
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useDaemonStore } from '@/stores/daemon';

const { status } = storeToRefs(useDaemonStore());

const view = computed(() => {
  const s = status.value;
  switch (s.state) {
    case 'online':
      return { dot: 'bg-status-ok', label: `Soulseek · ${s.username}` };
    case 'starting':
      return { dot: 'bg-status-busy animate-pulse', label: 'Starting slskd…' };
    case 'connecting':
      return { dot: 'bg-status-busy animate-pulse', label: 'Connecting to Soulseek…' };
    case 'error':
      return { dot: 'bg-status-error', label: 'Soulseek error' };
    default:
      return { dot: 'bg-status-idle', label: 'Soulseek offline' };
  }
});
</script>

<template>
  <router-link
    :to="{ path: '/settings', query: { tab: 'soulseek' } }"
    class="mx-2 flex items-center gap-2 rounded-md px-2.5 py-1.5 text-xs text-text-secondary hover:bg-white/5 hover:text-text-primary"
    :title="status.state === 'error' ? status.message : undefined"
  >
    <span class="size-2 shrink-0 rounded-full" :class="view.dot" />
    <span class="truncate">{{ view.label }}</span>
  </router-link>
</template>
