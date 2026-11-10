<script setup lang="ts">
import { onMounted, ref } from 'vue';
import TrackSummary from '@/components/TrackSummary.vue';
import { useJobsStore } from '@/stores/jobs';
import { usePlaylistStore } from '@/stores/playlist';
import { useSearchStore } from '@/stores/search';
import { formatDuration } from '@/composables/useFormat';
import type { Job } from '@/types/job';
import type { PlaylistEntry } from '@/types/playlist';
import { trackKey, type TrackRequest } from '@/types/track';

const props = defineProps<{ entry: PlaylistEntry; index: number }>();
const emit = defineEmits<{ started: [job: Job] }>();

const jobsStore = useJobsStore();
const playlistStore = usePlaylistStore();
const searchStore = useSearchStore();

const query = ref(`${props.entry.track.artists[0] ?? ''} ${props.entry.track.title}`.trim());
const results = ref<TrackRequest[]>([]);
const searching = ref(false);
const starting = ref(false);
const error = ref<string | null>(null);

async function search() {
  if (!query.value.trim()) return;
  searching.value = true;
  error.value = null;
  try {
    results.value = await searchStore.find(query.value.trim());
  } catch (e) {
    error.value = String(e);
  } finally {
    searching.value = false;
  }
}

/** Uses `request` for this playlist track and starts its download. */
async function choose(request: TrackRequest) {
  starting.value = true;
  error.value = null;
  try {
    await playlistStore.setMatch(props.index, request);
    emit('started', await jobsStore.start(request));
  } catch (e) {
    error.value = String(e);
  } finally {
    starting.value = false;
  }
}

onMounted(search);
</script>

<template>
  <div class="flex flex-col gap-5 p-6">
    <div>
      <h2 class="truncate text-lg font-semibold">{{ entry.track.title }}</h2>
      <div class="truncate text-sm text-text-secondary">{{ entry.track.artists.join(', ') }}</div>
      <div class="text-xs text-text-muted">On Spotify · {{ formatDuration(entry.track.durationSecs) }}</div>
    </div>

    <div v-if="entry.request" class="flex flex-col gap-1.5">
      <h3 class="section-label">Current match</h3>
      <div class="flex items-center gap-3">
        <TrackSummary :track="entry.request" />
        <button
          :disabled="starting"
          class="shrink-0 rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
          @click="choose(entry.request)"
        >
          Get lossless
        </button>
      </div>
    </div>
    <p v-else class="text-xs text-text-muted">
      No confident match was found. Pick the right version below, or edit the search.
    </p>

    <div class="flex flex-col gap-1.5">
      <h3 class="section-label">{{ entry.request ? 'Other versions' : 'Find a match' }}</h3>
      <form class="flex gap-2" @submit.prevent="search">
        <input
          v-model="query"
          type="search"
          spellcheck="false"
          class="min-w-0 flex-1 rounded border border-border bg-surface-1 px-3 py-1.5 text-sm outline-none focus:border-accent"
        />
        <button
          type="submit"
          :disabled="searching || !query.trim()"
          class="rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
        >
          {{ searching ? 'Searching…' : 'Search' }}
        </button>
      </form>
      <p v-if="error" class="error-box">{{ error }}</p>
      <ul v-if="results.length" class="divide-y divide-border">
        <li v-for="result in results" :key="trackKey(result)" class="flex items-center gap-3 py-2">
          <TrackSummary :track="result" cover-class="size-9" />
          <button
            :disabled="starting"
            class="shrink-0 rounded bg-surface-2 px-2 py-0.5 text-[11px] transition-colors hover:bg-surface-3 disabled:opacity-50"
            @click="choose(result)"
          >
            Use this
          </button>
        </li>
      </ul>
      <p v-else-if="!searching" class="text-xs text-text-muted">No results.</p>
    </div>
  </div>
</template>
