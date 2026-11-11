<script setup lang="ts">
import { computed } from 'vue';
import { basename, formatBytes, formatQuality, formatSpeed } from '@/composables/useFormat';
import type { Candidate } from '@/types/job';

const props = defineProps<{
  /** All of the job's candidates; indices refer to this list. */
  candidates: Candidate[];
  /** Show the files that failed a check instead of those that passed. */
  rejected?: boolean;
  activeIndex: number | null;
  completed: boolean;
  /** Disables manual downloads while the job is running. */
  disabled: boolean;
}>();

const emit = defineEmits<{ download: [index: number] }>();

const rows = computed(() =>
  props.candidates
    .map((candidate, index) => ({ c: candidate, index }))
    .filter(({ c }) => !!c.rejected === !!props.rejected),
);
</script>

<template>
  <!-- Two lines a file, to fit the side panel. -->
  <ul class="text-xs">
    <li
      v-for="{ c, index } in rows"
      :key="`${c.username}:${c.filename}`"
      class="flex items-center gap-2 border-b border-border/50 py-1.5"
      :class="{ 'text-text-muted': c.failed, 'bg-surface-1': index === activeIndex }"
    >
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="shrink-0 font-mono">{{ formatQuality(c.format, c.bitDepth, c.sampleRate, c.bitRate) }}</span>
          <span class="shrink-0 font-mono text-text-muted">{{ formatBytes(c.size) }}</span>
          <span class="min-w-0 truncate text-text-secondary" :title="`${c.username} · ${formatSpeed(c.uploadSpeed)}`">
            <span
              class="mr-1 inline-block size-1.5 rounded-full"
              :class="c.hasFreeUploadSlot ? 'bg-status-ok' : 'bg-status-busy'"
              :title="c.hasFreeUploadSlot ? 'Free upload slot' : 'No free upload slot'"
            />
            {{ c.username }}
          </span>
        </div>
        <div class="truncate text-[11px] text-text-muted" :title="c.filename">{{ basename(c.filename) }}</div>
        <div v-if="c.rejected" class="truncate text-[11px] text-status-busy" :title="c.rejected">{{ c.rejected }}</div>
      </div>
      <span v-if="index === activeIndex && completed" class="shrink-0 text-status-ok">Used</span>
      <span v-else-if="c.failed" class="shrink-0 text-status-error">Failed</span>
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
