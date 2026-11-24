<script setup lang="ts">
import { openUrl } from '@tauri-apps/plugin-opener';
import type { YoutubeMatch, YoutubeSource } from '@/types/job';

defineProps<{
  matches: YoutubeMatch[];
  /** The upload the job downloaded, or is downloading. */
  used: YoutubeSource | null;
  completed: boolean;
  disabled: boolean;
}>();

const emit = defineEmits<{ download: [index: number] }>();

function mmss(secs: number | null): string {
  return secs === null ? '' : `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
}
</script>

<template>
  <ul class="text-xs">
    <li
      v-for="(m, index) in matches"
      :key="m.videoId"
      class="flex items-center gap-2 border-b border-border/50 py-1.5"
      :class="{ 'text-text-muted': m.failed, 'bg-surface-1': used?.videoId === m.videoId }"
    >
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="shrink-0 font-mono">AAC</span>
          <span v-if="m.lengthSecs !== null" class="shrink-0 font-mono text-text-muted">{{ mmss(m.lengthSecs) }}</span>
          <button
            class="min-w-0 truncate text-left text-accent hover:text-accent-hover"
            :title="m.url"
            @click="openUrl(m.url)"
          >
            {{ m.title }}
          </button>
        </div>
        <div v-if="m.channel" class="truncate text-[11px] text-text-muted">{{ m.channel }}</div>
      </div>
      <span v-if="used?.videoId === m.videoId && completed" class="shrink-0 text-status-ok">Used</span>
      <span v-else-if="m.failed" class="shrink-0 text-status-error">Failed</span>
      <button
        v-else
        :disabled="disabled"
        class="shrink-0 rounded bg-surface-2 px-2 py-0.5 transition-colors hover:bg-surface-3 disabled:opacity-40"
        @click="emit('download', index)"
      >
        Download
      </button>
    </li>
  </ul>
</template>
