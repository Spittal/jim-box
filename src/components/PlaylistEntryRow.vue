<script setup lang="ts">
import { computed } from 'vue';
import CoverArt from '@/components/CoverArt.vue';
import { useJobsStore } from '@/stores/jobs';
import { formatDuration } from '@/composables/useFormat';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import type { PlaylistEntry } from '@/types/playlist';

const props = defineProps<{ entry: PlaylistEntry; selected: boolean; active: boolean }>();
const emit = defineEmits<{ toggle: []; open: [] }>();

const jobsStore = useJobsStore();
const request = computed(() => props.entry.request);
const job = computed(() => (request.value ? jobsStore.jobFor(request.value) : undefined));
</script>

<template>
  <li
    class="flex cursor-default items-center gap-3 border-b border-border px-4 py-2 transition-colors"
    :class="active ? 'bg-surface-2' : 'hover:bg-surface-1'"
    @click="emit('open')"
  >
    <input
      type="checkbox"
      class="size-3.5 shrink-0 accent-accent"
      :checked="selected && !job"
      :disabled="!request || !!job"
      @click.stop
      @change="emit('toggle')"
    />
    <CoverArt :src="request?.coverUrl ?? null" class="size-9" />
    <div class="min-w-0 flex-1">
      <div class="truncate text-sm">{{ entry.track.title }}</div>
      <div class="truncate text-xs text-text-muted">
        {{ entry.track.artists.join(', ') }} · {{ formatDuration(entry.track.durationSecs) }}
        <template v-if="request"> · {{ request.album }}</template>
      </div>
    </div>

    <JobStatusBadge
      v-if="job"
      :job="job"
      class="shrink-0 rounded border border-border px-2 py-1 text-xs text-text-secondary"
    />
    <span
      v-else-if="entry.confidence === 'medium'"
      class="shrink-0 rounded bg-status-busy/15 px-1.5 font-mono text-[10px] leading-4 text-status-busy"
      title="Title and artist match but the length differs a little. Check it's the right version."
    >check</span>
    <button
      v-else-if="!request"
      class="shrink-0 rounded bg-surface-2 px-2 py-0.5 text-[11px] transition-colors hover:bg-surface-3"
      title="No confident match. Pick one yourself."
      @click.stop="emit('open')"
    >
      Find match
    </button>
  </li>
</template>
