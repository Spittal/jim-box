<script setup lang="ts">
import { computed, onMounted } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { storeToRefs } from 'pinia';
import TrackResultRow from '@/components/TrackResultRow.vue';
import PlaylistResults from '@/components/PlaylistResults.vue';
import PlaylistEntryPicker from '@/components/PlaylistEntryPicker.vue';
import JobDetail from '@/components/JobDetail.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import CoverArt from '@/components/CoverArt.vue';
import { formatAgo } from '@/composables/useFormat';
import { useSearchStore } from '@/stores/search';
import { usePlaylistStore } from '@/stores/playlist';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import type { Job } from '@/types/job';
import { trackKey } from '@/types/track';

const route = useRoute();
const router = useRouter();
const searchStore = useSearchStore();
const { query, results, loading, error, searchedFor } = storeToRefs(searchStore);
const playlistStore = usePlaylistStore();
const { playlist, recent, loading: playlistLoading, progress, error: playlistError } = storeToRefs(playlistStore);
const { isOnline } = storeToRefs(useDaemonStore());
const jobsStore = useJobsStore();

const busy = computed(() => loading.value || playlistLoading.value);
const shownError = computed(() => error.value ?? playlistError.value);

// The side panel shows `?job=<id>` (a download) or `?entry=<index>` (a
// playlist track without one, to pick its match).
const selectedJob = computed(() => {
  const id = route.query.job;
  return typeof id === 'string' ? jobsStore.byId(id) : undefined;
});
const selectedEntryIndex = computed(() => {
  const index = Number(route.query.entry);
  return route.query.entry !== undefined && Number.isInteger(index) ? index : null;
});
const selectedEntry = computed(() =>
  selectedEntryIndex.value === null ? undefined : playlist.value?.entries[selectedEntryIndex.value],
);

function openJob(job: Job) {
  router.replace({ query: { job: job.id } });
}

function openEntry(index: number) {
  const entry = playlist.value?.entries[index];
  const job = entry?.request ? jobsStore.jobFor(entry.request) : undefined;
  router.replace({ query: job ? { job: job.id } : { entry: String(index) } });
}

function closePanel() {
  router.replace({ query: {} });
}

function search() {
  closePanel();
  searchStore.search();
}

function openPlaylist(id: string) {
  closePanel();
  playlistStore.open(id);
}

onMounted(() => playlistStore.loadRecent());
</script>

<template>
  <div class="flex h-full">
    <div class="flex min-w-0 flex-1 flex-col">
      <header class="border-b border-border px-4 py-3">
        <form class="flex gap-2" @submit.prevent="search">
          <input
            v-model="query"
            type="search"
            autofocus
            spellcheck="false"
            placeholder="Artist and track, or a Spotify playlist link"
            class="min-w-0 flex-1 rounded border border-border bg-surface-1 px-3 py-1.5 text-sm outline-none placeholder:text-text-muted focus:border-accent"
          />
          <button
            type="submit"
            :disabled="busy || !query.trim()"
            class="rounded bg-accent px-4 py-1.5 text-sm font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
          >
            {{ busy ? 'Searching…' : 'Search' }}
          </button>
        </form>
        <p v-if="!isOnline" class="mt-2 text-xs text-status-busy">
          Soulseek isn't connected yet. Downloads you start will wait for it.
        </p>
      </header>

      <p v-if="shownError" class="m-4 error-box">{{ shownError }}</p>

      <div v-if="playlistLoading" class="flex flex-col gap-2 p-8 text-center text-sm text-text-muted">
        <template v-if="progress?.total">
          Matching tracks… {{ progress.done }} of {{ progress.total }}
          <ProgressBar :percent="(progress.done / progress.total) * 100" class="mx-auto w-64" />
        </template>
        <template v-else>Reading playlist…</template>
      </div>

      <PlaylistResults
        v-else-if="playlist"
        :active-job-id="selectedJob?.id ?? null"
        :active-entry="selectedEntryIndex"
        @open="openEntry"
      />

      <div v-else class="min-h-0 flex-1 overflow-y-auto">
        <ul v-if="results.length" class="divide-y divide-border">
          <TrackResultRow
            v-for="track in results"
            :key="trackKey(track)"
            :track="track"
            :active="!!selectedJob && trackKey(selectedJob.request) === trackKey(track)"
            @open="openJob"
          />
        </ul>

        <p v-else-if="searchedFor && !loading && !shownError" class="p-8 text-center text-sm text-text-muted">
          No tracks found for “{{ searchedFor }}”.
        </p>

        <div v-else-if="!busy && !shownError" class="flex flex-col gap-6 p-8">
          <p class="text-center text-sm text-text-muted">
            Search for a track, pick the right version, and jim-box will fetch the best lossless
            copy on Soulseek, tag it from MusicBrainz, file it in your music folder and add it to your
            rekordbox collection. Paste a public
            Spotify playlist link to find a whole playlist at once.
          </p>
          <section v-if="recent.length" class="mx-auto flex w-full max-w-xl flex-col gap-1.5">
            <h2 class="section-label">Recent playlists</h2>
            <ul class="divide-y divide-border rounded border border-border">
              <li
                v-for="item in recent"
                :key="item.id"
                class="group flex cursor-default items-center gap-3 px-3 py-2 transition-colors hover:bg-surface-1"
                @click="openPlaylist(item.id)"
              >
                <CoverArt :src="item.coverUrl" class="size-9" />
                <div class="min-w-0 flex-1">
                  <div class="truncate text-sm">{{ item.name }}</div>
                  <div class="text-xs text-text-muted">
                    {{ item.tracks }} tracks · {{ item.matched }} matched · {{ formatAgo(item.loadedAt) }}
                  </div>
                </div>
                <button
                  class="shrink-0 rounded px-2 py-0.5 text-xs text-text-muted opacity-0 transition group-hover:opacity-100 hover:text-status-error"
                  title="Forget this playlist"
                  @click.stop="playlistStore.remove(item.id)"
                >
                  Remove
                </button>
              </li>
            </ul>
          </section>
        </div>
      </div>
    </div>

    <aside v-if="selectedJob || selectedEntry" class="w-[28rem] shrink-0 overflow-y-auto border-l border-border">
      <div class="sticky top-0 z-10 flex justify-end bg-surface-0 px-2 pt-2">
        <button
          class="rounded px-2 py-0.5 text-sm text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
          title="Close"
          @click="closePanel"
        >
          ✕
        </button>
      </div>
      <JobDetail v-if="selectedJob" :key="selectedJob.id" :job="selectedJob" @removed="closePanel" />
      <PlaylistEntryPicker
        v-else-if="selectedEntry && selectedEntryIndex !== null"
        :key="selectedEntryIndex"
        :entry="selectedEntry"
        :index="selectedEntryIndex"
        @started="openJob"
      />
    </aside>
  </div>
</template>
