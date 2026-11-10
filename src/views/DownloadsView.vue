<script setup lang="ts">
import { computed } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { storeToRefs } from 'pinia';
import JobListItem from '@/components/JobListItem.vue';
import JobDetail from '@/components/JobDetail.vue';
import { useJobsStore } from '@/stores/jobs';

const route = useRoute();
const router = useRouter();
const jobsStore = useJobsStore();
const { jobs } = storeToRefs(jobsStore);

const selected = computed(() => {
  const id = route.params.id;
  return typeof id === 'string' ? jobsStore.byId(id) : undefined;
});

const notFoundCount = computed(() => jobs.value.filter((j) => j.status === 'notFound').length);

const hasClearable = computed(() =>
  jobs.value.some((j) => j.status === 'completed' || j.status === 'cancelled'),
);
</script>

<template>
  <div class="flex h-full">
    <section class="flex w-80 shrink-0 flex-col border-r border-border">
      <header class="flex items-center justify-between border-b border-border px-4 py-3">
        <h1 class="text-sm font-medium">Downloads</h1>
        <div class="flex gap-3">
          <button
            v-if="notFoundCount"
            class="text-xs text-text-muted transition-colors hover:text-text-secondary"
            @click="jobsStore.retryNotFound()"
          >
            Retry {{ notFoundCount }} not found
          </button>
          <button
            v-if="hasClearable"
            class="text-xs text-text-muted transition-colors hover:text-text-secondary"
            @click="jobsStore.clearFinished()"
          >
            Clear finished
          </button>
        </div>
      </header>
      <ul v-if="jobs.length" class="min-h-0 flex-1 overflow-y-auto">
        <JobListItem v-for="job in jobs" :key="job.id" :job="job" />
      </ul>
      <p v-else class="p-6 text-center text-sm text-text-muted">
        Nothing here yet. Find a track in
        <router-link to="/search" class="text-accent hover:text-accent-hover">Search</router-link>.
      </p>
    </section>

    <section class="min-w-0 flex-1 overflow-y-auto">
      <JobDetail v-if="selected" :key="selected.id" :job="selected" @removed="router.push('/downloads')" />
      <p v-else class="p-8 text-center text-sm text-text-muted">
        {{ jobs.length ? 'Select a download to see its details.' : '' }}
      </p>
    </section>
  </div>
</template>
