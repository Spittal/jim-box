<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { formatAgo } from '@/composables/useFormat';
import { useTrackActions } from '@/composables/useTrackActions';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { usePlaylistStore } from '@/stores/playlist';
import { shortcut } from '@/platform';

/** The heading of the list being shown, and what can be done to it or the tracks selected in it. */
const store = useCollectionStore();
const { status, playlists, waiting, analysis, loading, error } = storeToRefs(store);
const browse = useBrowseStore();
const { selected, selectedNode, inPlaylist, selection, total, notice, link, pendingRows, pendingSelection, selectedPending } =
  storeToRefs(browse);
const libraryStore = useLibraryStore();
const playlistStore = usePlaylistStore();
const jobs = useJobsStore();
const actions = useTrackActions();

const heading = computed(() => selectedNode.value?.name ?? 'Collection');
const countLabel = computed(() => `${total.value} ${total.value === 1 ? 'track' : 'tracks'}`);

/** Runs something that reports failure by throwing, showing why it failed. */
async function run(action: () => Promise<unknown>) {
  error.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

// --- The selected tracks ----------------------------------------------------

const targetPlaylists = computed(() =>
  playlists.value.filter((p) => p.kind === 'playlist' && p.id !== selected.value),
);

function addToChosen(event: Event) {
  const select = event.target as HTMLSelectElement;
  if (select.value) actions.addToPlaylist(select.value);
  select.value = '';
}

// --- The selected pending rows -----------------------------------------------

/** Of the pending rows picked: those with a fetch, those whose fetch stopped short, and those not fetched yet. */
const pickedFetches = computed(() => selectedPending.value.filter((r) => r.job));
const pickedStopped = computed(() =>
  selectedPending.value.filter((r) => r.job && !isActive(r.job) && r.job.status !== 'completed'),
);
const pickedUnfetched = computed(() => selectedPending.value.filter((r) => !r.job && r.entry?.request));

const working = ref(false);

/** Does something to each of some rows, carrying on past the ones that fail. */
function each<T>(rows: T[], action: (row: T) => Promise<unknown>) {
  working.value = true;
  error.value = null;
  (async () => {
    let failed: string | null = null;
    for (const row of rows) {
      try {
        await action(row);
      } catch (e) {
        failed = String(e);
      }
    }
    error.value = failed;
  })().finally(() => (working.value = false));
}

/** Gives up on the picked fetches: stops the ones running, and takes them off the list. */
function removeFetches() {
  const rows = pickedFetches.value;
  browse.clearSelection();
  each(rows, (row) => jobs.discard(row.job!.id));
}
const retryPicked = () => each(pickedStopped.value, (row) => jobs.retry(row.job!.id));
const fetchPicked = () => each(pickedUnfetched.value, (row) => jobs.fetch(row.entry!.request!, selected.value));

// Analysis: BPM, beat grid, key and waveforms.

async function analyseAll() {
  const queued = await store.analyse(null, false);
  if (queued === 0) notice.value = 'Every track in the collection is analysed.';
}

// Adding music from disk: to the collection, and to the open playlist.

async function addFromDisk(directory: boolean) {
  const picked = await openDialog({
    directory,
    multiple: true,
    title: directory ? 'Add folders to the collection' : 'Add files to the collection',
    filters: directory
      ? undefined
      : [{ name: 'Music', extensions: ['mp3', 'm4a', 'aac', 'flac', 'wav', 'aiff', 'aif', 'ogg', 'opus'] }],
  });
  const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
  if (!paths.length) return;
  notice.value = null;
  const report = await store.importPaths(paths, inPlaylist.value ? selected.value : null);
  if (!report) return;
  const parts = [`Added ${report.added} ${report.added === 1 ? 'track' : 'tracks'}`];
  if (report.existing) parts.push(`${report.existing} already in the collection`);
  if (report.skipped.length) parts.push(`${report.skipped.length} couldn't be read`);
  notice.value = `${parts.join(' · ')}.`;
}

// --- A playlist that came from Spotify ---------------------------------------

/** Missing tracks with a sure match and no fetch yet. */
const fetchable = computed(() =>
  link.value ? pendingRows.value.filter((r) => r.entry?.request && r.entry.confidence === 'high' && !r.job) : [],
);
const fetchingAll = ref(false);

function fetchAll() {
  fetchingAll.value = true;
  // Fetches queue in the backend (a few run at a time), so starting them all is fine.
  run(async () => {
    for (const row of fetchable.value) await jobs.fetch(row.entry!.request!, selected.value);
  }).finally(() => (fetchingAll.value = false));
}

const linkClass = 'text-[11px] text-text-muted transition-colors hover:text-text-primary';
const buttonClass =
  'rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <header class="flex flex-col gap-2 border-b border-border px-4 py-3">
    <div class="flex items-center justify-between gap-4">
      <div class="min-w-0">
        <h1 class="truncate text-sm font-medium">{{ heading }}</h1>
        <p class="truncate font-mono text-xs text-text-muted">
          <template v-if="status">{{ countLabel }} · {{ status.masterDb }}</template>
          <template v-else-if="loading">Reading your rekordbox library…</template>
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <span
          v-if="status?.rekordboxRunning"
          class="rounded bg-status-busy/15 px-2 py-0.5 text-[11px] text-status-busy"
          title="rekordbox (or its background agent) is running. Its library can be read, but not changed, until you quit it."
        >rekordbox is open: read-only</span>
        <button
          v-if="waiting.count"
          class="rounded bg-status-busy/15 px-2 py-0.5 text-[11px] text-status-busy hover:bg-status-busy/25"
          :title="`${waiting.error ?? 'Waiting for rekordbox to close.'} Click to try adding them now.`"
          @click="store.open()"
        >{{ waiting.count }} {{ waiting.count === 1 ? 'file waits' : 'files wait' }} to be added</button>
        <template v-if="selection.size">
          <span class="text-[11px] text-text-muted">{{ selection.size }} selected</span>
          <button
            v-if="selection.size < total"
            :class="linkClass"
            :title="`Select every track in this list (${shortcut('A')})`"
            @click="browse.selectAll()"
          >
            Select all {{ total }}
          </button>
          <button :class="linkClass" title="Select nothing" @click="browse.clearSelection()">Clear</button>
          <button
            :class="buttonClass"
            :disabled="libraryStore.busy"
            title="Look the selected tracks up on MusicBrainz and propose tags and file names to review"
            @click="actions.identify()"
          >
            Identify
          </button>
          <button
            :class="buttonClass"
            title="Work out the BPM, beat grid, key and waveforms of the selected tracks that don't have them"
            @click="actions.analyse()"
          >
            Analyse
          </button>
          <button
            :class="buttonClass"
            :disabled="!actions.reviewable().length"
            :title="
              actions.allReviewed()
                ? 'Take the review back: the Grid column shows how sure the analysis was again'
                : 'You\'ve checked the selected tracks\' beat grids against the music and they\'re right. Analysing a batch of tracks again leaves reviewed grids alone.'
            "
            @click="actions.review()"
          >
            {{ actions.allReviewed() ? 'Not reviewed' : 'Reviewed' }}
          </button>
          <select
            v-if="targetPlaylists.length"
            class="max-w-40 rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none"
            @change="addToChosen"
          >
            <option value="">Add to playlist…</option>
            <option v-for="p in targetPlaylists" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
          <button
            v-if="inPlaylist"
            :class="buttonClass"
            title="Take the selected tracks out of this playlist. They stay in the collection."
            @click="actions.removeFromPlaylist()"
          >
            Remove from playlist
          </button>
          <button
            :class="buttonClass"
            title="Take the selected tracks out of the collection and every playlist. You're asked first, and whether their files go too."
            @click="actions.remove()"
          >
            Delete…
          </button>
        </template>
        <template v-else-if="pendingSelection.size">
          <span class="text-[11px] text-text-muted">{{ selectedPending.length }} selected</span>
          <button
            v-if="selectedPending.length < pendingRows.length"
            :class="linkClass"
            :title="`Select every track that isn't in the collection yet (${shortcut('A')})`"
            @click="browse.selectAll()"
          >
            Select all {{ pendingRows.length }}
          </button>
          <button :class="linkClass" title="Select nothing" @click="browse.clearSelection()">Clear</button>
          <button
            v-if="pickedUnfetched.length"
            :class="buttonClass"
            :disabled="working"
            title="Find the best lossless copy of each and add it to the collection and this playlist"
            @click="fetchPicked"
          >
            Get lossless for {{ pickedUnfetched.length }}
          </button>
          <button
            v-if="pickedStopped.length"
            :class="buttonClass"
            :disabled="working"
            title="Search again for the selected tracks that found no source or failed"
            @click="retryPicked"
          >
            Search again for {{ pickedStopped.length }}
          </button>
          <button
            v-if="pickedFetches.length"
            :class="buttonClass"
            :disabled="working"
            title="Give up on the selected fetches: the ones running are stopped, and they leave this list. Nothing in the collection changes."
            @click="removeFetches"
          >
            Remove {{ pickedFetches.length }}
          </button>
        </template>
        <template v-else>
          <button
            :class="buttonClass"
            :disabled="analysis?.active"
            title="Work out the BPM, beat grid, key and waveforms of every track in the collection that doesn't have them"
            @click="analyseAll"
          >
            Analyse new
          </button>
          <button
            :class="buttonClass"
            :title="inPlaylist ? 'Add music files to the collection and this playlist' : 'Add music files to the collection'"
            @click="addFromDisk(false)"
          >
            Add files…
          </button>
          <button
            :class="buttonClass"
            :title="inPlaylist ? 'Add all the music in a folder to the collection and this playlist' : 'Add all the music in a folder to the collection'"
            @click="addFromDisk(true)"
          >
            Add folder…
          </button>
        </template>
        <button
          :class="buttonClass"
          :disabled="loading"
          title="Read the library again to pick up changes made in rekordbox"
          @click="store.open()"
        >
          Refresh
        </button>
      </div>
    </div>

    <!-- A playlist that follows a Spotify playlist: what it still lacks. -->
    <div v-if="link" class="flex items-center gap-3 text-xs text-text-secondary">
      <span class="min-w-0 flex-1 truncate">
        Follows Spotify · {{ link.total - link.missing.length }} of {{ link.total }} tracks in your collection ·
        synced {{ formatAgo(link.loadedAt) }}
        <template v-if="status?.rekordboxRunning"> · changes to this playlist wait for rekordbox to close</template>
        <template v-if="link.truncated">
          · Spotify only shares the first 100 tracks of a playlist without signing in, so tracks past that aren't followed
        </template>
      </span>
      <button
        v-if="fetchable.length"
        class="shrink-0 rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
        :disabled="fetchingAll"
        title="Fetch every missing track whose match is sure and that isn't being fetched. The others are left for you to check."
        @click="fetchAll"
      >
        {{ fetchingAll ? 'Queuing…' : `Get lossless for ${fetchable.length} missing` }}
      </button>
      <button
        class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
        :disabled="playlistStore.importing"
        title="Spotify is read every minute while jim-box is open. This reads it now, and looks again for tracks that found no match."
        @click="run(playlistStore.refresh)"
      >
        {{ playlistStore.importing ? 'Reading…' : 'Sync now' }}
      </button>
    </div>
  </header>
</template>
