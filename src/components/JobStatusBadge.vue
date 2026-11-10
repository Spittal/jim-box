<script setup lang="ts">
import { statusColor, statusLabel } from '@/composables/useJobStatus';
import type { Job } from '@/types/job';

defineProps<{ job: Job }>();
</script>

<template>
  <span class="flex items-center gap-1.5">
    <span class="size-1.5 shrink-0 rounded-full" :class="statusColor(job.status)" />
    <span class="truncate">{{ statusLabel(job) }}</span>
  </span>
</template>
