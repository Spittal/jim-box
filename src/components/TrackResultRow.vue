<script setup lang="ts">
import { computed, ref } from 'vue';
import { useJobsStore } from '@/stores/jobs';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import TrackSummary from '@/components/TrackSummary.vue';
import type { Job } from '@/types/job';
import type { TrackRequest } from '@/types/track';

const props = defineProps<{ track: TrackRequest; active: boolean }>();
const emit = defineEmits<{ open: [job: Job] }>();

const jobsStore = useJobsStore();
const job = computed(() => jobsStore.jobFor(props.track));
const starting = ref(false);
const error = ref<string | null>(null);

async function download() {
  starting.value = true;
  error.value = null;
  try {
    emit('open', await jobsStore.start(props.track));
  } catch (e) {
    error.value = String(e);
  } finally {
    starting.value = false;
  }
}
</script>

<template>
  <li
    class="flex items-center gap-3 px-4 py-2 transition-colors"
    :class="active ? 'bg-surface-2' : 'hover:bg-surface-1'"
    @click="job && emit('open', job)"
  >
    <TrackSummary :track="track" />
    <span v-if="error" class="shrink-0 text-xs text-status-error">{{ error }}</span>

    <JobStatusBadge
      v-if="job"
      :job="job"
      class="shrink-0 rounded border border-border px-2 py-1 text-xs text-text-secondary"
    />
    <button
      v-else
      :disabled="starting"
      class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs font-medium text-text-primary transition-colors hover:bg-surface-3 disabled:opacity-50"
      @click="download"
    >
      Get lossless
    </button>
  </li>
</template>
