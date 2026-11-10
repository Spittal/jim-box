<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import { dragging, dropRow, endDrag, startDrag } from '@/composables/useDrag';
import { formatDuration } from '@/composables/useFormat';
import { progressPercent } from '@/composables/useJobStatus';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { useBrowseStore, type PendingRow } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import type { CollectionTrack, TrackSort } from '@/types/collection';

/**
 * The tracks of the list being shown. Tracks still being fetched come first,
 * dimmed, with where their fetch has got to; then the ones in the collection.
 */
const emit = defineEmits<{ open: [track: CollectionTrack] }>();

const store = useCollectionStore();
const { analysis } = storeToRefs(store);
const browse = useBrowseStore();
const { sort, descending, search, total, listError, selection, selectedIds, pendingRows, pendingSelection, pendingOnly, inPlaylist } =
  storeToRefs(browse);
const jobs = useJobsStore();
const library = useLibraryStore();

// Rows are fetched a page at a time as they scroll into view. Only the
// visible slice is in the DOM, so a large library scrolls like a small one.
const ROW_HEIGHT = 28;
const OVERSCAN = 12;

const scroller = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewportHeight = ref(600);

/** The pending rows sit above the collection's, and scroll away with them. */
const pendingHeight = computed(() => pendingRows.value.length * ROW_HEIGHT);
const listTop = computed(() => Math.max(0, scrollTop.value - pendingHeight.value));

const firstVisible = computed(() => Math.max(0, Math.floor(listTop.value / ROW_HEIGHT) - OVERSCAN));
const lastVisible = computed(() =>
  Math.min(total.value, Math.ceil((listTop.value + viewportHeight.value) / ROW_HEIGHT) + OVERSCAN),
);

onMounted(() => browse.ensure(firstVisible.value, lastVisible.value));
watch([firstVisible, lastVisible], ([first, last]) => browse.ensure(first, last));
watch(
  () => browse.resets,
  () => {
    scrollTop.value = 0;
    if (scroller.value) scroller.value.scrollTop = 0;
  },
);

const visibleRows = computed(() => {
  const rows: { index: number; track: CollectionTrack | null }[] = [];
  for (let i = firstVisible.value; i < lastVisible.value; i++) rows.push({ index: i, track: browse.trackAt(i) });
  return rows;
});

function onScroll() {
  if (!scroller.value) return;
  scrollTop.value = scroller.value.scrollTop;
  viewportHeight.value = scroller.value.clientHeight;
}

// Keep the viewport height current as the list appears and the window resizes.
const resizeObserver = new ResizeObserver(onScroll);
watch(scroller, (el) => {
  resizeObserver.disconnect();
  if (el) resizeObserver.observe(el);
});
onUnmounted(() => resizeObserver.disconnect());

// --- Selection --------------------------------------------------------------

/**
 * Shift- and ⌘-clicks pick rows; they don't select text. Without this, WebKit
 * stretches a text selection from wherever the caret last was to the click.
 */
function keepTextUnselected(event: MouseEvent) {
  if (!event.shiftKey && !event.metaKey) return;
  event.preventDefault();
  window.getSelection()?.removeAllRanges();
}

/** Where a shift-click's range starts. */
let anchor: number | null = null;

function clickRow(index: number, track: CollectionTrack, event: MouseEvent) {
  const next = new Set(event.metaKey || event.shiftKey ? selection.value : []);
  if (event.shiftKey && anchor !== null) {
    const [from, to] = anchor < index ? [anchor, index] : [index, anchor];
    for (let i = from; i <= to; i++) {
      const id = browse.trackAt(i)?.id;
      if (id) next.add(id);
    }
  } else if (event.metaKey && next.has(track.id)) {
    next.delete(track.id);
    anchor = index;
  } else {
    next.add(track.id);
    anchor = index;
  }
  browse.selectTracks(next);
}

// --- Tracks on their way ----------------------------------------------------

/** Where a shift-click's range of pending rows starts. */
let pendingAnchor: string | null = null;

/** Picks pending rows as tracks are picked: ⌘ adds or drops one, shift takes a range. */
function clickPending(row: PendingRow, event?: MouseEvent) {
  const rows = pendingRows.value;
  const next = new Set(event?.metaKey || event?.shiftKey ? pendingSelection.value : []);
  const from = rows.findIndex((r) => r.key === pendingAnchor);
  if (event?.shiftKey && from !== -1) {
    const to = rows.indexOf(row);
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) next.add(rows[i].key);
  } else if (event?.metaKey && next.has(row.key)) {
    next.delete(row.key);
    pendingAnchor = row.key;
  } else {
    next.add(row.key);
    pendingAnchor = row.key;
  }
  browse.selectPending(next);
}

const rowError = ref<{ key: string; message: string } | null>(null);

async function act(row: PendingRow, action: () => Promise<unknown>) {
  rowError.value = null;
  try {
    await action();
  } catch (e) {
    rowError.value = { key: row.key, message: String(e) };
  }
}

/** Fetches a Spotify playlist's track into the collection and this playlist. */
const fetchEntry = (row: PendingRow) => act(row, () => jobs.fetch(row.entry!.request!, browse.selected));
const retry = (row: PendingRow) => act(row, () => jobs.retry(row.job!.id));

// --- Reordering a playlist ---------------------------------------------------

/** Tracks can be dragged into a new order only in the playlist's own order. */
const reorderable = computed(
  () => inPlaylist.value && sort.value === 'trackNo' && !descending.value && !search.value.trim() && !pendingOnly.value,
);

function dragTrack(event: DragEvent, track: CollectionTrack) {
  // Dragging an unselected row drags just that row.
  if (!selection.value.has(track.id)) browse.selectTracks([track.id]);
  startDrag(event, { kind: 'tracks', ids: selectedIds.value });
}

function overRow(event: DragEvent, index: number) {
  if (dragging.value?.kind !== 'tracks' || !reorderable.value) return;
  event.preventDefault();
  dropRow.value = index;
}

/** `index` is the row the tracks go before; the list's length for the end. */
async function dropOnRow(index: number) {
  const drag = dragging.value;
  endDrag();
  if (drag?.kind !== 'tracks' || !reorderable.value || !browse.selected) return;
  const before = index < total.value ? (browse.trackAt(index)?.id ?? null) : null;
  await store.edit({ kind: 'moveTracks', playlistId: browse.selected, trackIds: drag.ids, before });
}

// --- Display ----------------------------------------------------------------

const columns: { id: TrackSort; label: string; class: string }[] = [
  { id: 'trackNo', label: '#', class: 'w-10 text-right' },
  { id: 'title', label: 'Title', class: 'min-w-0 flex-[3]' },
  { id: 'artist', label: 'Artist', class: 'min-w-0 flex-[2]' },
  { id: 'album', label: 'Album', class: 'min-w-0 flex-[2]' },
  { id: 'genre', label: 'Genre', class: 'min-w-0 flex-1' },
  { id: 'bpm', label: 'BPM', class: 'w-14 text-right' },
  { id: 'key', label: 'Key', class: 'w-10' },
  { id: 'duration', label: 'Time', class: 'w-12 text-right' },
  { id: 'rating', label: 'Rating', class: 'w-16' },
  { id: 'dateAdded', label: 'Added', class: 'w-24' },
];

const smallButtonClass =
  'shrink-0 rounded bg-surface-2 px-2 py-0.5 text-[11px] text-text-primary transition-colors hover:bg-surface-3';
</script>

<template>
  <div class="flex min-h-0 min-w-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center gap-3 border-b border-border px-4 py-1.5 text-[11px] text-text-muted">
      <span class="w-1.5 shrink-0" />
      <button
        v-for="column in columns"
        :key="column.id"
        class="shrink-0 truncate text-left hover:text-text-secondary"
        :class="[column.class, { 'text-text-primary': sort === column.id }]"
        @click="browse.sortBy(column.id)"
      >
        {{ column.label }}<span v-if="sort === column.id">{{ descending ? ' ↓' : ' ↑' }}</span>
      </button>
    </div>
    <p v-if="listError" class="error-box m-4">{{ listError }}</p>
    <div
      ref="scroller"
      class="min-h-0 flex-1 overflow-y-auto select-none"
      @scroll.passive="onScroll"
      @mousedown="keepTextUnselected"
      @dragover="overRow($event, total)"
      @drop.prevent="dropOnRow(total)"
    >
      <!-- Tracks on their way. The last cells line up under BPM to Added. -->
      <div
        v-for="row in pendingRows"
        :key="row.key"
        class="flex cursor-default items-center gap-3 px-4 text-xs text-text-secondary"
        :class="pendingSelection.has(row.key) ? 'bg-surface-2' : 'hover:bg-surface-1'"
        :style="{ height: `${ROW_HEIGHT}px` }"
        @click="clickPending(row, $event)"
      >
        <span class="w-1.5 shrink-0" />
        <span class="w-10 shrink-0 text-right font-mono text-text-muted" title="Not in the collection yet">↓</span>
        <span class="min-w-0 flex-[3] truncate">{{ row.title }}</span>
        <span class="min-w-0 flex-[2] truncate text-text-muted">{{ row.artist }}</span>
        <span class="min-w-0 flex-[2] truncate text-text-muted">{{ row.album }}</span>
        <span class="min-w-0 flex-1">
          <ProgressBar v-if="row.job && progressPercent(row.job) !== null" :percent="progressPercent(row.job)!" />
        </span>
        <span class="flex w-[22rem] shrink-0 items-center gap-2">
          <span
            v-if="rowError?.key === row.key"
            class="min-w-0 flex-1 truncate text-status-error"
            :title="rowError.message"
          >{{ rowError.message }}</span>
          <template v-else-if="row.job">
            <JobStatusBadge :job="row.job" class="min-w-0 flex-1" :title="row.job.error ?? undefined" />
            <button
              v-if="!isActive(row.job) && row.job.status !== 'completed'"
              :class="smallButtonClass"
              title="Search for it again"
              @click.stop="retry(row)"
            >
              Search again
            </button>
            <span
              v-else-if="row.job.status === 'completed'"
              class="shrink-0 text-[11px] text-text-muted"
              title="rekordbox is open, so its library can't be changed. The track joins the collection when you quit rekordbox."
            >waits for rekordbox to close</span>
          </template>
          <template v-else-if="row.entry?.request">
            <span class="min-w-0 flex-1 truncate text-text-muted">
              On Spotify · {{ formatDuration(row.durationSecs) }}
            </span>
            <span
              v-if="row.entry.confidence === 'medium'"
              class="shrink-0 rounded bg-status-busy/15 px-1.5 font-mono text-[10px] leading-4 text-status-busy"
              title="Title and artist match but the length differs a little. Click the row to check it's the right version."
            >check</span>
            <button
              :class="smallButtonClass"
              title="Find the best lossless copy and add it to the collection and this playlist"
              @click.stop="fetchEntry(row)"
            >
              Get lossless
            </button>
          </template>
          <template v-else>
            <span class="min-w-0 flex-1 truncate text-text-muted">On Spotify · no confident match</span>
            <button :class="smallButtonClass" title="Pick the right version yourself" @click.stop="clickPending(row)">
              Find match
            </button>
          </template>
        </span>
      </div>

      <div v-if="!pendingOnly" class="relative" :style="{ height: `${total * ROW_HEIGHT}px` }">
        <div
          v-for="row in visibleRows"
          :key="row.index"
          class="absolute right-0 left-0 flex cursor-default items-center gap-3 px-4 text-xs"
          :class="[
            row.track && selection.has(row.track.id) ? 'bg-surface-2' : 'hover:bg-surface-1',
            dropRow === row.index ? 'border-t border-accent' : '',
          ]"
          :style="{ top: `${row.index * ROW_HEIGHT}px`, height: `${ROW_HEIGHT}px` }"
          :draggable="!!row.track"
          @click="row.track && clickRow(row.index, row.track, $event)"
          @dblclick="row.track && emit('open', row.track)"
          @dragstart="row.track && dragTrack($event, row.track)"
          @dragend="endDrag"
          @dragover.stop="overRow($event, row.index)"
          @drop.prevent.stop="dropOnRow(row.index)"
        >
          <template v-if="row.track">
            <span
              class="size-1.5 shrink-0 rounded-full"
              :style="{ backgroundColor: TRACK_COLORS[row.track.color]?.hex || 'transparent' }"
            />
            <span class="w-10 shrink-0 text-right font-mono text-text-muted">{{ row.track.position }}</span>
            <span class="flex min-w-0 flex-[3] items-center gap-1.5">
              <span class="truncate" :title="row.track.path">{{ row.track.title }}</span>
              <span
                v-if="library.forTrack(row.track.id)?.status === 'matched'"
                class="shrink-0 rounded bg-accent/15 px-1.5 font-mono text-[10px] leading-4 text-accent-hover"
                title="Identifying found tags or a file name to fix. Select the track to review it."
              >fix</span>
              <span
                v-if="jobs.replacing.has(row.track.path)"
                class="shrink-0 font-mono text-[10px] text-text-muted"
                title="A new copy of this track is being fetched"
              >↓</span>
            </span>
            <span class="min-w-0 flex-[2] truncate text-text-secondary">{{ row.track.artist }}</span>
            <span class="min-w-0 flex-[2] truncate text-text-secondary">{{ row.track.album }}</span>
            <span class="min-w-0 flex-1 truncate text-text-muted">{{ row.track.genre }}</span>
            <span class="w-14 shrink-0 text-right font-mono">
              <span v-if="analysis?.running.includes(row.track.id)" class="text-text-muted" title="Being analysed">…</span>
              <template v-else>{{ row.track.bpm?.toFixed(1) ?? '' }}</template>
            </span>
            <span class="w-10 shrink-0 font-mono text-text-secondary">{{ row.track.key }}</span>
            <span class="w-12 shrink-0 text-right font-mono text-text-muted">
              {{ row.track.durationSecs ? formatDuration(row.track.durationSecs) : '' }}
            </span>
            <span class="w-16 shrink-0 text-[10px] tracking-tight text-text-secondary">{{ '★'.repeat(row.track.rating) }}</span>
            <span class="w-24 shrink-0 font-mono text-text-muted">{{ row.track.dateAdded.slice(0, 10) }}</span>
          </template>
        </div>
      </div>
      <p
        v-if="!listError && !pendingRows.length && (pendingOnly || !total)"
        class="p-8 text-center text-sm text-text-muted"
      >
        <template v-if="search.trim()">
          Nothing here matches. Press Return to look for “{{ search.trim() }}” online.
        </template>
        <template v-else-if="pendingOnly">Nothing is being fetched.</template>
        <template v-else-if="inPlaylist">
          Nothing here yet. Drag tracks onto this playlist, or search below and add what you find.
        </template>
        <template v-else>No tracks here.</template>
      </p>
    </div>
  </div>
</template>
