<script setup lang="ts">
import { computed } from 'vue';
import CoverArt from '@/components/CoverArt.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import { progressPercent } from '@/composables/useJobStatus';
import type { Job } from '@/types/job';

const props = defineProps<{ job: Job }>();
const percent = computed(() => progressPercent(props.job));
</script>

<template>
  <li>
    <router-link
      :to="`/downloads/${job.id}`"
      class="flex items-center gap-3 border-b border-border px-4 py-2 transition-colors hover:bg-surface-1"
      active-class="bg-surface-2"
    >
      <CoverArt :src="job.request.coverUrl" class="size-9" />
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm">{{ job.request.title }}</div>
        <div class="truncate text-xs text-text-muted">{{ job.request.artist }}</div>
        <JobStatusBadge :job="job" class="mt-0.5 text-[11px] text-text-secondary" />
        <ProgressBar v-if="percent !== null" :percent="percent" class="mt-1" />
      </div>
    </router-link>
  </li>
</template>
