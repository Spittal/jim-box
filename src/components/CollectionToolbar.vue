<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { formatAgo, formatLength } from '@/composables/useFormat';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { usePlaylistStore } from '@/stores/playlist';
import { shortcut } from '@/platform';

/** The heading of the list being shown, what narrows it, and what can be added to it. */
const store = useCollectionStore();
const { status, waiting, loading, error } = storeToRefs(store);
const browse = useBrowseStore();
const {
  selected,
  selectedNode,
  inPlaylist,
  selection,
  total,
  notice,
  link,
  pendingRows,
  pendingSelection,
  selectedPending,
  reviewedFilter,
  genreFilter,
  ratingFilter,
  filterCount,
  lists,
  totalSecs,
} = storeToRefs(browse);
const playlistStore = usePlaylistStore();
const jobs = useJobsStore();

/** The names of the lists being shown together, when there are several. */
const listNames = computed(() =>
  lists.value.length > 1 ? lists.value.map((id) => store.playlists.find((p) => p.id === id)?.name ?? '').join(', ') : '',
);
const heading = computed(() => selectedNode.value?.name ?? (listNames.value || 'Collection'));
/** How many tracks, and how long they'd play one after another. */
const countLabel = computed(() => {
  const count = `${total.value} ${total.value === 1 ? 'track' : 'tracks'}`;
  return total.value && totalSecs.value ? `${count} · ${formatLength(totalSecs.value)}` : count;
});

/** Runs something that reports failure by throwing, showing why it failed. */
async function run(action: () => Promise<unknown>) {
  error.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

// --- Filtering the list -------------------------------------------------------

const filterMenu = ref(false);
/** Every genre in the collection, the most used first; read as the menu opens. */
const genres = ref<string[]>([]);
/** Narrows the genres on offer. */
const genreSearch = ref('');

const reviewChoices: { label: string; value: boolean | null }[] = [
  { label: 'All', value: null },
  { label: 'Reviewed', value: true },
  { label: 'Not reviewed', value: false },
];

/** The genres to tick: the ones picked (still listed if no track has them now), then the rest. */
const offeredGenres = computed(() => {
  const typed = genreSearch.value.trim().toLowerCase();
  const all = [...new Set([...genreFilter.value.filter(Boolean), ...genres.value])];
  return typed ? all.filter((name) => name.toLowerCase().includes(typed)) : all;
});

async function toggleFilterMenu() {
  filterMenu.value = !filterMenu.value;
  if (!filterMenu.value) return;
  genreSearch.value = '';
  genres.value = await store.namesOf('genre');
}

function toggleRating(stars: number) {
  ratingFilter.value = ratingFilter.value.includes(stars)
    ? ratingFilter.value.filter((r) => r !== stars)
    : [...ratingFilter.value, stars];
}

function toggleGenre(name: string) {
  genreFilter.value = genreFilter.value.includes(name)
    ? genreFilter.value.filter((g) => g !== name)
    : [...genreFilter.value, name];
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

// --- The library's own menu ------------------------------------------------------

const libraryMenu = ref(false);

/** Closes the menu and does what was picked from it. */
function pickFromMenu(action: () => unknown) {
  libraryMenu.value = false;
  void action();
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
const menuItemClass = 'px-3 py-1 text-left hover:bg-surface-2 disabled:opacity-50 disabled:hover:bg-transparent';
const buttonClass =
  'rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <header class="flex flex-col gap-2 border-b border-border px-4 py-3">
    <div class="flex items-center gap-3">
      <!-- What's done to the library as a whole: adding to it, reading it again. -->
      <div class="relative shrink-0">
        <button
          class="flex size-7 items-center justify-center rounded bg-surface-2 text-sm leading-none transition-colors hover:bg-surface-3"
          :class="libraryMenu ? 'bg-surface-3' : ''"
          title="Add music, refresh"
          aria-label="Library actions"
          aria-haspopup="menu"
          :aria-expanded="libraryMenu"
          @click="libraryMenu = !libraryMenu"
        >
          ⋯
        </button>
        <template v-if="libraryMenu">
          <div class="fixed inset-0 z-10" @click="libraryMenu = false" @contextmenu.prevent.stop="libraryMenu = false" />
          <div
            role="menu"
            class="absolute top-full left-0 z-20 mt-1 flex w-44 flex-col rounded border border-border bg-surface-1 py-1 text-xs text-text-primary shadow-lg"
            @contextmenu.prevent.stop
            @keydown.esc.stop="libraryMenu = false"
          >
            <button
              role="menuitem"
              :class="menuItemClass"
              :title="inPlaylist ? 'Add music files to the collection and this playlist' : 'Add music files to the collection'"
              @click="pickFromMenu(() => addFromDisk(false))"
            >
              Add files…
            </button>
            <button
              role="menuitem"
              :class="menuItemClass"
              :title="inPlaylist ? 'Add all the music in a folder to the collection and this playlist' : 'Add all the music in a folder to the collection'"
              @click="pickFromMenu(() => addFromDisk(true))"
            >
              Add folder…
            </button>
            <button
              role="menuitem"
              :class="menuItemClass"
              class="mt-1 border-t border-border pt-1.5"
              :disabled="loading"
              title="Read the library again to pick up changes made in rekordbox"
              @click="pickFromMenu(() => store.open())"
            >
              Refresh
            </button>
          </div>
        </template>
      </div>
      <div class="min-w-0 flex-1">
        <h1 class="truncate text-sm font-medium" :title="listNames || undefined">{{ heading }}</h1>
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
        <div class="relative">
          <button
            :class="[buttonClass, filterCount ? 'text-accent' : '']"
            title="Show only some of this list's tracks"
            @click="toggleFilterMenu"
          >
            Filter<template v-if="filterCount"> · {{ filterCount }}</template> ▾
          </button>
          <template v-if="filterMenu">
            <div class="fixed inset-0 z-10" @click="filterMenu = false" @contextmenu.prevent.stop="filterMenu = false" />
            <div
              class="absolute top-full right-0 z-20 mt-1 flex w-52 flex-col rounded border border-border bg-surface-1 py-1 text-xs text-text-primary shadow-lg"
              @contextmenu.prevent.stop
              @keydown.esc.stop="filterMenu = false"
            >
              <p class="px-3 pt-1 pb-0.5 text-[10px] tracking-wide text-text-muted uppercase">Beat grid</p>
              <label
                v-for="choice in reviewChoices"
                :key="choice.label"
                class="flex cursor-default items-center gap-2 px-3 py-1 hover:bg-surface-2"
              >
                <input
                  type="radio"
                  name="filter-reviewed"
                  :checked="reviewedFilter === choice.value"
                  @change="reviewedFilter = choice.value"
                />
                {{ choice.label }}
              </label>
              <p class="mt-1 border-t border-border px-3 pt-2 pb-0.5 text-[10px] tracking-wide text-text-muted uppercase">
                Rating
              </p>
              <label
                v-for="stars in [5, 4, 3, 2, 1, 0]"
                :key="stars"
                class="flex cursor-default items-center gap-2 px-3 py-0.5 hover:bg-surface-2"
                :class="stars ? '' : 'text-text-secondary'"
              >
                <input type="checkbox" :checked="ratingFilter.includes(stars)" @change="toggleRating(stars)" />
                <span v-if="stars" :aria-label="`${stars} ${stars === 1 ? 'star' : 'stars'}`">
                  {{ '★'.repeat(stars) }}<span class="text-text-muted/40">{{ '★'.repeat(5 - stars) }}</span>
                </span>
                <template v-else>No stars</template>
              </label>
              <p class="mt-1 border-t border-border px-3 pt-2 pb-0.5 text-[10px] tracking-wide text-text-muted uppercase">
                Genre
              </p>
              <input
                v-if="genres.length > 8"
                v-model="genreSearch"
                class="mx-3 my-1 rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none placeholder:text-text-muted focus:border-border-active"
                placeholder="Find a genre…"
                spellcheck="false"
                autocomplete="off"
              />
              <div class="max-h-64 overflow-y-auto">
                <label
                  v-if="!genreSearch.trim()"
                  class="flex cursor-default items-center gap-2 px-3 py-1 text-text-secondary hover:bg-surface-2"
                >
                  <input type="checkbox" :checked="genreFilter.includes('')" @change="toggleGenre('')" />
                  No genre
                </label>
                <label
                  v-for="name in offeredGenres"
                  :key="name"
                  class="flex cursor-default items-center gap-2 px-3 py-1 hover:bg-surface-2"
                >
                  <input type="checkbox" :checked="genreFilter.includes(name)" @change="toggleGenre(name)" />
                  <span class="truncate">{{ name }}</span>
                </label>
              </div>
              <button
                class="mt-1 border-t border-border px-3 pt-1.5 pb-0.5 text-left text-text-muted hover:text-text-primary disabled:opacity-50"
                :disabled="!filterCount"
                @click="browse.clearFilters()"
              >
                Clear filters
              </button>
            </div>
          </template>
        </div>
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
