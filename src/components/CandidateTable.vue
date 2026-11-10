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
  <table class="w-full table-fixed text-left text-xs">
    <thead class="text-text-muted">
      <tr class="border-b border-border">
        <th class="w-28 py-1.5 font-normal">Quality</th>
        <th class="w-20 py-1.5 font-normal">Size</th>
        <th class="py-1.5 font-normal">File</th>
        <th class="w-36 py-1.5 font-normal">Peer</th>
        <th class="w-24 py-1.5" />
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="{ c, index } in rows"
        :key="`${c.username}:${c.filename}`"
        class="border-b border-border/50"
        :class="{ 'text-text-muted': c.failed, 'bg-surface-1': index === activeIndex }"
      >
        <td class="py-1.5 font-mono">{{ formatQuality(c.format, c.bitDepth, c.sampleRate, c.bitRate) }}</td>
        <td class="py-1.5 font-mono">{{ formatBytes(c.size) }}</td>
        <td class="py-1.5 pr-2" :title="c.filename">
          <div class="truncate">{{ basename(c.filename) }}</div>
          <div v-if="c.rejected" class="truncate text-[11px] text-status-busy">{{ c.rejected }}</div>
        </td>
        <td class="truncate py-1.5 pr-2" :title="`${c.username} · ${formatSpeed(c.uploadSpeed)}`">
          <span
            class="mr-1 inline-block size-1.5 rounded-full"
            :class="c.hasFreeUploadSlot ? 'bg-status-ok' : 'bg-status-busy'"
            :title="c.hasFreeUploadSlot ? 'Free upload slot' : `${c.queueLength} queued`"
          />
          {{ c.username }}
        </td>
        <td class="py-1.5 text-right">
          <span v-if="index === activeIndex && completed" class="text-status-ok">Used</span>
          <span v-else-if="c.failed" class="text-status-error">Failed</span>
          <button
            v-else
            :disabled="disabled"
            class="rounded bg-surface-2 px-2 py-0.5 transition-colors hover:bg-surface-3 disabled:opacity-40"
            @click="emit('download', index)"
          >
            Download
          </button>
        </td>
      </tr>
    </tbody>
  </table>
</template>
