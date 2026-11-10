<script setup lang="ts">
import { ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import TrackSummary from '@/components/TrackSummary.vue';
import { endDrag, startDrag } from '@/composables/useDrag';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import { useSearchStore } from '@/stores/search';
import type { Job } from '@/types/job';
import { trackKey, type TrackRequest } from '@/types/track';

/**
 * Tracks found online, above the search bar. One you have can go into the
 * open playlist; one you don't is fetched into it. Either can be dragged onto
 * any playlist.
 */
const searchStore = useSearchStore();
const { open, results, owned, loading, error, searchedFor } = storeToRefs(searchStore);
const browse = useBrowseStore();
const { targetPlaylist } = storeToRefs(browse);
const collection = useCollectionStore();
const jobs = useJobsStore();
const { isOnline } = storeToRefs(useDaemonStore());

// What's in the collection changes as tracks land and are removed.
watch(
  () => collection.generation,
  () => open.value && searchStore.refreshOwned(),
);

/** The fetch to show for a result: one on its way, or one that stopped short. */
function jobOf(track: TrackRequest): Job | null {
  const job = jobs.jobFor(track);
  return job && job.status !== 'cancelled' ? job : null;
}

/** Results put in a playlist from here, as "result key > playlist id". */
const added = ref(new Set<string>());
const rowError = ref<{ key: string; message: string } | null>(null);
watch(results, () => {
  added.value = new Set();
  rowError.value = null;
});

const addedKey = (track: TrackRequest) => `${trackKey(track)}>${targetPlaylist.value?.id}`;

async function addOwned(track: TrackRequest, trackId: string) {
  const playlist = targetPlaylist.value;
  if (!playlist) return;
  const key = addedKey(track);
  const done = await collection.edit({ kind: 'addTracks', playlistId: playlist.id, trackIds: [trackId] });
  if (done !== undefined) added.value = new Set(added.value).add(key);
}

async function fetch(track: TrackRequest) {
  rowError.value = null;
  try {
    await jobs.fetch(track, targetPlaylist.value?.id);
  } catch (e) {
    rowError.value = { key: trackKey(track), message: String(e) };
  }
}

/** Shows an owned result in the list's detail panel. */
function showOwned(trackId: string) {
  browse.selectTracks([trackId]);
}

const buttonClass =
  'shrink-0 rounded bg-surface-2 px-3 py-1 text-xs font-medium text-text-primary transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <section v-if="open" class="flex max-h-[45%] shrink-0 flex-col border-t border-border-active bg-surface-1">
    <header class="flex shrink-0 items-center gap-3 border-b border-border px-4 py-1.5 text-xs text-text-muted">
      <span class="min-w-0 flex-1 truncate">
        <template v-if="loading">Looking online…</template>
        <template v-else-if="searchedFor && !error">
          Online, for “{{ searchedFor }}”
          <template v-if="results.length"> · drag a result onto a playlist, or use its button</template>
        </template>
      </span>
      <span v-if="!isOnline" class="shrink-0 text-status-busy">
        Soulseek isn't connected yet. Tracks you fetch will wait for it.
      </span>
      <button
        class="shrink-0 rounded px-2 text-sm text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
        title="Close (Esc)"
        @click="searchStore.close()"
      >
        ✕
      </button>
    </header>

    <p v-if="error" class="error-box m-3">{{ error }}</p>
    <ul v-else-if="results.length" class="min-h-0 flex-1 divide-y divide-border overflow-y-auto">
      <li
        v-for="(track, index) in results"
        :key="trackKey(track)"
        class="flex cursor-default items-center gap-3 px-4 py-1.5 transition-colors hover:bg-surface-2"
        draggable="true"
        @dragstart="startDrag($event, { kind: 'result', track, ownedId: owned[index] ?? null })"
        @dragend="endDrag"
      >
        <TrackSummary :track="track" cover-class="size-9" />
        <span v-if="rowError?.key === trackKey(track)" class="max-w-64 shrink-0 truncate text-xs text-status-error" :title="rowError.message">
          {{ rowError.message }}
        </span>

        <template v-if="owned[index]">
          <button
            class="shrink-0 text-xs text-status-ok hover:underline"
            title="You have this track. Click to select it."
            @click="showOwned(owned[index]!)"
          >
            In your collection
          </button>
          <span v-if="targetPlaylist && added.has(addedKey(track))" class="shrink-0 text-xs text-text-muted">
            Added to {{ targetPlaylist.name }}
          </span>
          <button v-else-if="targetPlaylist" :class="buttonClass" @click="addOwned(track, owned[index]!)">
            Add to {{ targetPlaylist.name }}
          </button>
        </template>
        <button
          v-else-if="jobOf(track)"
          class="shrink-0 rounded border border-border px-2 py-1 text-xs text-text-secondary hover:border-border-active"
          title="Show how its fetch is going"
          @click="browse.showPending({ kind: 'job', id: jobOf(track)!.id })"
        >
          <JobStatusBadge :job="jobOf(track)!" />
        </button>
        <button
          v-else
          :class="buttonClass"
          :title="targetPlaylist
            ? `Find the best lossless copy and add it to the collection and “${targetPlaylist.name}”`
            : 'Find the best lossless copy and add it to the collection'"
          @click="fetch(track)"
        >
          Get lossless
        </button>
      </li>
    </ul>
    <p v-else-if="!loading && searchedFor" class="p-6 text-center text-sm text-text-muted">
      No tracks found for “{{ searchedFor }}”.
    </p>
  </section>
</template>
