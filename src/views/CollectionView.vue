<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useRouter } from 'vue-router';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import CollectionTrackDetail from '@/components/CollectionTrackDetail.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import TrackPlayer from '@/components/TrackPlayer.vue';
import { formatDuration } from '@/composables/useFormat';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { useCollectionStore } from '@/stores/collection';
import { useLibraryStore } from '@/stores/library';
import type {
  CollectionTrack,
  PlaylistNode,
  TrackQuery,
  TrackSort,
  TrackSource,
} from '@/types/collection';

const store = useCollectionStore();
const { status, playlists, waiting, analysis, loading, error, generation } = storeToRefs(store);
const libraryStore = useLibraryStore();
const router = useRouter();

onMounted(() => {
  if (!status.value && !loading.value) store.open();
  store.loadAnalysis();
});

// --- Playlist tree ----------------------------------------------------------

const selected = ref<string | null>(null);
const collapsed = ref(new Set<string>());

/** Tree rows, without the descendants of collapsed folders. */
const treeRows = computed(() => {
  const rows: PlaylistNode[] = [];
  let hiddenBelow: number | null = null;
  for (const node of playlists.value) {
    if (hiddenBelow !== null) {
      if (node.depth > hiddenBelow) continue;
      hiddenBelow = null;
    }
    rows.push(node);
    if (node.kind === 'folder' && collapsed.value.has(node.id)) hiddenBelow = node.depth;
  }
  return rows;
});

function toggleFolder(id: string) {
  const next = new Set(collapsed.value);
  if (!next.delete(id)) next.add(id);
  collapsed.value = next;
}

const selectedNode = computed(() => playlists.value.find((p) => p.id === selected.value) ?? null);
// A playlist deleted (here or in rekordbox) falls back to the collection.
watch(playlists, () => {
  if (selected.value && !selectedNode.value) select(null);
});

// Creating, renaming and deleting playlists.

/** The tree row whose name is being typed: a new one, or one being renamed. */
const naming = ref<{ id: string | null; folder: boolean; parentId: string | null; name: string } | null>(null);
const nameInput = ref<HTMLInputElement[] | HTMLInputElement | null>(null);
const confirmingDelete = ref<string | null>(null);

async function focusNameInput() {
  await nextTick();
  const input = Array.isArray(nameInput.value) ? nameInput.value[0] : nameInput.value;
  input?.focus();
  input?.select();
}

/** New playlists go in the selected folder, or beside the selected playlist. */
function startCreate(folder: boolean) {
  const node = selectedNode.value;
  const parentId = node ? (node.kind === 'folder' ? node.id : node.parentId) : null;
  if (parentId && collapsed.value.has(parentId)) toggleFolder(parentId);
  naming.value = { id: null, folder, parentId, name: folder ? 'New folder' : 'New playlist' };
  focusNameInput();
}

function startRename(node: PlaylistNode) {
  naming.value = { id: node.id, folder: node.kind === 'folder', parentId: node.parentId, name: node.name };
  focusNameInput();
}

async function commitName() {
  const pending = naming.value;
  if (!pending) return;
  naming.value = null;
  const name = pending.name.trim();
  if (!name) return;
  if (pending.id) {
    if (playlists.value.find((p) => p.id === pending.id)?.name !== name) {
      await store.edit({ kind: 'renamePlaylist', id: pending.id, name });
    }
  } else {
    const id = await store.edit({
      kind: 'createPlaylist',
      name,
      parentId: pending.parentId,
      folder: pending.folder,
    });
    if (id) select(id);
  }
}

async function deletePlaylist(id: string) {
  confirmingDelete.value = null;
  await store.edit({ kind: 'deletePlaylist', id });
}

/** Where a new row's name box goes: after its parent's last descendant. */
const newRowAfter = computed(() => {
  const pending = naming.value;
  if (!pending || pending.id) return undefined;
  if (!pending.parentId) return treeRows.value.length ? treeRows.value[treeRows.value.length - 1].id : null;
  const rows = treeRows.value;
  const parentAt = rows.findIndex((r) => r.id === pending.parentId);
  if (parentAt === -1) return null;
  let last = parentAt;
  while (last + 1 < rows.length && rows[last + 1].depth > rows[parentAt].depth) last++;
  return rows[last].id;
});
const newRowDepth = computed(() => {
  const parent = playlists.value.find((p) => p.id === naming.value?.parentId);
  return parent ? parent.depth + 1 : 0;
});

// --- Track list -------------------------------------------------------------

const search = ref('');
const sort = ref<TrackSort>('dateAdded');
const descending = ref(true);

/** A playlist opens in its own order; the collection, newest first. */
function select(id: string | null) {
  selected.value = id;
  confirmingDelete.value = null;
  const ordered = id !== null && playlists.value.find((p) => p.id === id)?.kind === 'playlist';
  sort.value = ordered ? 'trackNo' : 'dateAdded';
  descending.value = !ordered;
}

const query = computed<TrackQuery>(() => {
  const source: TrackSource = selected.value
    ? { kind: 'playlist', id: selected.value }
    : { kind: 'collection' };
  return { source, sort: sort.value, descending: descending.value, query: search.value };
});

function sortBy(column: TrackSort) {
  if (sort.value === column) descending.value = !descending.value;
  else {
    sort.value = column;
    descending.value = column === 'dateAdded' || column === 'rating';
  }
}

// Rows are fetched a page at a time as they scroll into view. Only the
// visible slice is in the DOM, so a large library scrolls like a small one.
const ROW_HEIGHT = 28;
const PAGE = 200;
const OVERSCAN = 12;

const total = ref(0);
const pages = shallowRef(new Map<number, CollectionTrack[]>());
const requested = new Set<number>();
const listError = ref<string | null>(null);
const scroller = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewportHeight = ref(600);
/** Identifies the list being shown, so late answers for an old one are dropped. */
let listVersion = 0;

async function fetchPage(page: number) {
  if (requested.has(page)) return;
  requested.add(page);
  const version = listVersion;
  try {
    const result = await store.tracks(query.value, page * PAGE, PAGE);
    if (version !== listVersion) return;
    total.value = result.total;
    const next = new Map(pages.value);
    next.set(page, result.rows);
    pages.value = next;
    listError.value = null;
  } catch (e) {
    if (version !== listVersion) return;
    requested.delete(page);
    listError.value = String(e);
  }
}

const firstVisible = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_HEIGHT) - OVERSCAN));
const lastVisible = computed(() =>
  Math.min(total.value, Math.ceil((scrollTop.value + viewportHeight.value) / ROW_HEIGHT) + OVERSCAN),
);

function fetchVisible() {
  const first = Math.floor(firstVisible.value / PAGE);
  const last = Math.floor(Math.max(firstVisible.value, lastVisible.value - 1) / PAGE);
  for (let page = first; page <= last; page++) fetchPage(page);
}

/** A different list: start again from the top. */
function resetList() {
  listVersion++;
  requested.clear();
  pages.value = new Map();
  total.value = 0;
  scrollTop.value = 0;
  selection.value = new Set();
  if (scroller.value) scroller.value.scrollTop = 0;
  if (status.value) fetchPage(0);
}

/** The same list after a change: fetch it again where it is, without a flash. */
function refreshList() {
  listVersion++;
  requested.clear();
  if (status.value) fetchVisible();
}

watch(query, resetList, { deep: true });
watch(generation, refreshList);
onMounted(resetList);
watch([firstVisible, lastVisible], () => {
  if (total.value) fetchVisible();
});

function trackAt(index: number): CollectionTrack | null {
  return pages.value.get(Math.floor(index / PAGE))?.[index % PAGE] ?? null;
}

const visibleRows = computed(() => {
  const rows: { index: number; track: CollectionTrack | null }[] = [];
  for (let i = firstVisible.value; i < lastVisible.value; i++) rows.push({ index: i, track: trackAt(i) });
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

const selection = ref(new Set<string>());
/** Where a shift-click's range starts. */
let anchor: number | null = null;

function clickRow(index: number, track: CollectionTrack, event: MouseEvent) {
  const next = new Set(event.metaKey || event.shiftKey ? selection.value : []);
  if (event.shiftKey && anchor !== null) {
    const [from, to] = anchor < index ? [anchor, index] : [index, anchor];
    for (let i = from; i <= to; i++) {
      const id = trackAt(i)?.id;
      if (id) next.add(id);
    }
  } else if (event.metaKey && next.has(track.id)) {
    next.delete(track.id);
    anchor = index;
  } else {
    next.add(track.id);
    anchor = index;
  }
  selection.value = next;
}

const selectedIds = computed(() => [...selection.value]);

/** The one selected track, for the detail panel; fresh after each change. */
const detailTrack = computed(() => {
  if (selection.value.size !== 1) return null;
  const id = selectedIds.value[0];
  for (const rows of pages.value.values()) {
    const found = rows.find((r) => r.id === id);
    if (found) return found;
  }
  return null;
});

/** The track in the player, as it was when opened. */
const opened = shallowRef<CollectionTrack | null>(null);
/** The same track as the list now has it, when it's loaded: fresh after edits. */
const playerTrack = computed(() => {
  const id = opened.value?.id;
  if (!id) return null;
  for (const rows of pages.value.values()) {
    const found = rows.find((r) => r.id === id);
    if (found) return found;
  }
  return opened.value;
});

// --- Changing playlists' tracks ----------------------------------------------

const inPlaylist = computed(() => selectedNode.value?.kind === 'playlist');
/** Tracks can be dragged into a new order only in the playlist's own order. */
const reorderable = computed(
  () => inPlaylist.value && sort.value === 'trackNo' && !descending.value && !search.value.trim(),
);
const targetPlaylists = computed(() =>
  playlists.value.filter((p) => p.kind === 'playlist' && p.id !== selected.value),
);

async function addTo(playlistId: string, trackIds = selectedIds.value) {
  if (trackIds.length) await store.edit({ kind: 'addTracks', playlistId, trackIds });
}

function addToChosen(event: Event) {
  const select = event.target as HTMLSelectElement;
  if (select.value) addTo(select.value);
  select.value = '';
}

async function removeSelected() {
  if (!selected.value || !selectedIds.value.length) return;
  const trackIds = selectedIds.value;
  selection.value = new Set();
  await store.edit({ kind: 'removeTracks', playlistId: selected.value, trackIds });
}

/** Asked before taking tracks out of the collection: how many. */
const confirmingRemoval = ref(0);
watch(selection, () => (confirmingRemoval.value = 0));

async function deleteSelected() {
  const trackIds = selectedIds.value;
  selection.value = new Set();
  await store.edit({ kind: 'deleteTracks', trackIds });
}

/** Looks the selected tracks up on MusicBrainz; what it finds is reviewed elsewhere. */
async function identifySelected() {
  try {
    await libraryStore.identify(selectedIds.value);
    router.push('/review');
  } catch (e) {
    error.value = String(e);
  }
}

// Analysis: BPM, beat grid, key and waveforms.

/** The selection size a re-analysis is being confirmed for; 0 when it isn't. */
const confirmingAnalysis = ref(0);
watch(selection, () => (confirmingAnalysis.value = 0));

/** Analyses what in the selection has no analysis; offers to redo it when all of it has. */
async function analyseSelected(again: boolean) {
  confirmingAnalysis.value = 0;
  const queued = await store.analyse(selectedIds.value, again);
  if (queued === 0 && !again) confirmingAnalysis.value = selection.value.size;
}

async function analyseAll() {
  const queued = await store.analyse(null, false);
  if (queued === 0) notice.value = 'Every track in the collection is analysed.';
}

const analysisPercent = computed(() =>
  analysis.value?.total ? (analysis.value.done / analysis.value.total) * 100 : 0,
);
const showFailures = ref(false);

// Adding music from disk: to the collection, and to the open playlist.

const notice = ref<string | null>(null);

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

// Drag and drop: tracks onto a playlist or into a new place in this one,
// and playlists into folders or a new place among their siblings.

type Drag = { kind: 'tracks'; ids: string[] } | { kind: 'playlist'; id: string };
const dragging = ref<Drag | null>(null);
/** The row a drop would land on: a tree row's id, or a track row's index. */
const dropTree = ref<string | null>(null);
const dropRow = ref<number | null>(null);

function startDrag(event: DragEvent, drag: Drag) {
  dragging.value = drag;
  // WebKit only starts a drag that carries data.
  event.dataTransfer?.setData('text/plain', drag.kind);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'copyMove';
}

function dragTrack(event: DragEvent, track: CollectionTrack) {
  // Dragging an unselected row drags just that row.
  if (!selection.value.has(track.id)) selection.value = new Set([track.id]);
  startDrag(event, { kind: 'tracks', ids: selectedIds.value });
}

function endDrag() {
  dragging.value = null;
  dropTree.value = null;
  dropRow.value = null;
}

/** Whether `node` is `ancestorId` or inside it. */
function isWithin(node: PlaylistNode, ancestorId: string): boolean {
  for (let at: PlaylistNode | undefined = node; at; at = playlists.value.find((p) => p.id === at!.parentId)) {
    if (at.id === ancestorId) return true;
  }
  return false;
}

function acceptsDrop(node: PlaylistNode): boolean {
  const drag = dragging.value;
  if (!drag) return false;
  if (drag.kind === 'tracks') return node.kind === 'playlist' && node.id !== selected.value;
  // A folder can't go inside itself.
  return !isWithin(node, drag.id);
}

function overTree(event: DragEvent, node: PlaylistNode) {
  if (!acceptsDrop(node)) return;
  event.preventDefault();
  dropTree.value = node.id;
}

async function dropOnTree(node: PlaylistNode) {
  const drag = dragging.value;
  const accepted = acceptsDrop(node);
  endDrag();
  if (!drag || !accepted) return;
  if (drag.kind === 'tracks') {
    await addTo(node.id, drag.ids);
  } else if (node.kind === 'folder') {
    await store.edit({ kind: 'movePlaylist', id: drag.id, parentId: node.id, index: null });
  } else {
    // Onto a playlist: take its place among its siblings.
    const siblings = playlists.value.filter((p) => p.parentId === node.parentId && p.id !== drag.id);
    await store.edit({
      kind: 'movePlaylist',
      id: drag.id,
      parentId: node.parentId,
      index: siblings.findIndex((p) => p.id === node.id),
    });
  }
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
  if (drag?.kind !== 'tracks' || !reorderable.value || !selected.value) return;
  const before = index < total.value ? (trackAt(index)?.id ?? null) : null;
  await store.edit({ kind: 'moveTracks', playlistId: selected.value, trackIds: drag.ids, before });
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

const heading = computed(() => selectedNode.value?.name ?? 'Collection');
const countLabel = computed(() => `${total.value} ${total.value === 1 ? 'track' : 'tracks'}`);

const buttonClass =
  'rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
const treeButtonClass = 'rounded px-1.5 py-0.5 text-[11px] text-text-muted hover:bg-surface-2 hover:text-text-primary';
const nameInputClass =
  'min-w-0 flex-1 rounded border border-border-active bg-surface-1 px-1 py-0 text-xs outline-none';
</script>

<template>
  <div class="flex h-full flex-col">
    <TrackPlayer :track="playerTrack" @close="opened = null" />
    <header class="flex items-center justify-between gap-4 border-b border-border px-4 py-3">
      <div class="min-w-0">
        <h1 class="text-sm font-medium">{{ heading }}</h1>
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
            :class="buttonClass"
            title="Look the selected tracks up on MusicBrainz and propose tags and file names to review"
            @click="identifySelected"
          >
            Identify
          </button>
          <template v-if="confirmingAnalysis === selection.size">
            <button
              :class="buttonClass"
              title="Cues are kept. Beat grids are replaced, including any adjusted by hand in rekordbox."
              @click="analyseSelected(true)"
            >
              Already analysed. Replace {{ selection.size === 1 ? 'its beat grid' : 'their beat grids' }}
            </button>
            <button :class="buttonClass" @click="confirmingAnalysis = 0">Cancel</button>
          </template>
          <button
            v-else
            :class="buttonClass"
            title="Work out the BPM, beat grid, key and waveforms of the selected tracks that don't have them"
            @click="analyseSelected(false)"
          >
            Analyse
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
            @click="removeSelected"
          >
            Remove from playlist
          </button>
          <template v-if="confirmingRemoval === selection.size">
            <button :class="buttonClass" class="text-status-error" @click="deleteSelected">
              Remove {{ selection.size }} from collection
            </button>
            <button :class="buttonClass" @click="confirmingRemoval = 0">Cancel</button>
          </template>
          <button
            v-else
            :class="buttonClass"
            title="Take the selected tracks out of the collection and every playlist. Their files stay on disk."
            @click="confirmingRemoval = selection.size"
          >
            Remove…
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
        <input
          v-model="search"
          type="search"
          spellcheck="false"
          placeholder="Search"
          class="w-56 rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none focus:border-border-active"
        />
        <button
          :class="buttonClass"
          :disabled="loading"
          title="Read the library again to pick up changes made in rekordbox"
          @click="store.open()"
        >
          Refresh
        </button>
      </div>
    </header>

    <p v-if="error" class="error-box mx-4 mt-3">{{ error }}</p>
    <p v-else-if="notice" class="mx-4 mt-3 text-xs text-text-secondary">
      {{ notice }}
      <button class="ml-1 text-text-muted hover:text-text-primary" title="Dismiss" @click="notice = null">✕</button>
    </p>

    <div v-if="analysis?.total || analysis?.stopped" class="mx-4 mt-3 flex flex-col gap-1.5 text-xs">
      <div class="flex items-center gap-3">
        <span class="shrink-0 text-text-secondary">
          <template v-if="analysis.active">Analysing {{ analysis.done }} of {{ analysis.total }}</template>
          <template v-else>Analysed {{ analysis.done - analysis.failed.length }} {{ analysis.done - analysis.failed.length === 1 ? 'track' : 'tracks' }}</template>
        </span>
        <ProgressBar v-if="analysis.active" :percent="analysisPercent" class="flex-1" />
        <button
          v-if="analysis.failed.length"
          class="shrink-0 text-status-error hover:underline"
          @click="showFailures = !showFailures"
        >
          {{ analysis.failed.length }} failed
        </button>
        <button v-if="analysis.active" :class="buttonClass" class="shrink-0" @click="store.cancelAnalysis()">
          Stop
        </button>
        <button
          v-else
          class="text-text-muted hover:text-text-primary"
          title="Dismiss"
          @click="analysis = null"
        >✕</button>
      </div>
      <p v-if="analysis.stopped" class="text-status-busy">{{ analysis.stopped }}</p>
      <ul v-if="showFailures && analysis.failed.length" class="error-box max-h-32 overflow-y-auto">
        <li v-for="f in analysis.failed" :key="f.trackId">{{ f.title }}: {{ f.error }}</li>
      </ul>
    </div>

    <div v-if="status" class="flex min-h-0 flex-1">
      <!-- Playlist tree -->
      <div class="flex w-56 shrink-0 flex-col border-r border-border">
        <ul class="min-h-0 flex-1 overflow-y-auto py-1 text-xs">
          <li
            class="flex cursor-default items-center gap-1 py-1 pr-2 pl-4"
            :class="selected === null ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1'"
            @click="select(null)"
          >
            <span class="min-w-0 flex-1 truncate">Collection</span>
            <span class="shrink-0 font-mono text-[10px] text-text-muted">{{ status.tracks }}</span>
          </li>
          <li class="section-label px-4 pt-3 pb-1 text-[10px]">Playlists</li>
          <li v-if="naming && !naming.id && newRowAfter === null" class="flex items-center py-0.5 pr-2 pl-4">
            <input ref="nameInput" v-model="naming.name" :class="nameInputClass" spellcheck="false" @keydown.enter="commitName" @keydown.esc="naming = null" @blur="commitName" />
          </li>
          <template v-for="node in treeRows" :key="node.id">
            <li
              class="group flex cursor-default items-center gap-1 py-1 pr-2"
              :class="[
                selected === node.id ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1',
                dropTree === node.id ? 'outline outline-1 -outline-offset-1 outline-accent' : '',
              ]"
              :style="{ paddingLeft: `${node.depth * 12 + 4}px` }"
              :draggable="naming?.id !== node.id"
              @click="select(node.id)"
              @dblclick="startRename(node)"
              @dragstart="startDrag($event, { kind: 'playlist', id: node.id })"
              @dragend="endDrag"
              @dragover="overTree($event, node)"
              @dragleave="dropTree === node.id && (dropTree = null)"
              @drop.prevent="dropOnTree(node)"
            >
              <button
                class="w-3 shrink-0 text-[9px] text-text-muted"
                :class="{ invisible: node.kind !== 'folder' }"
                @click.stop="toggleFolder(node.id)"
              >
                {{ collapsed.has(node.id) ? '▶' : '▼' }}
              </button>
              <input
                v-if="naming?.id === node.id"
                ref="nameInput"
                v-model="naming.name"
                :class="nameInputClass"
                spellcheck="false"
                @click.stop
                @dblclick.stop
                @keydown.enter="commitName"
                @keydown.esc="naming = null"
                @blur="commitName"
              />
              <template v-else>
                <span class="min-w-0 flex-1 truncate" :class="{ 'font-medium': node.kind === 'folder' }" :title="node.name">
                  {{ node.name }}
                </span>
                <span v-if="node.kind === 'smart'" class="shrink-0 text-[10px] text-text-muted" title="Intelligent playlist">auto</span>
                <span v-else-if="node.count !== null" class="shrink-0 font-mono text-[10px] text-text-muted">{{ node.count }}</span>
              </template>
            </li>
            <li
              v-if="naming && !naming.id && newRowAfter === node.id"
              class="flex items-center py-0.5 pr-2"
              :style="{ paddingLeft: `${newRowDepth * 12 + 16}px` }"
            >
              <input ref="nameInput" v-model="naming.name" :class="nameInputClass" spellcheck="false" @keydown.enter="commitName" @keydown.esc="naming = null" @blur="commitName" />
            </li>
          </template>
          <li v-if="!playlists.length && !naming" class="px-4 py-2 text-text-muted">No playlists yet.</li>
        </ul>
        <div class="flex shrink-0 flex-wrap items-center gap-1 border-t border-border p-1.5">
          <button :class="treeButtonClass" title="New playlist" @click="startCreate(false)">+ Playlist</button>
          <button :class="treeButtonClass" title="New folder" @click="startCreate(true)">+ Folder</button>
          <template v-if="selectedNode">
            <template v-if="confirmingDelete === selectedNode.id">
              <span class="px-1 text-[11px] text-text-secondary">Delete “{{ selectedNode.name }}”?</span>
              <button :class="treeButtonClass" class="text-status-error" @click="deletePlaylist(selectedNode.id)">Delete</button>
              <button :class="treeButtonClass" @click="confirmingDelete = null">Cancel</button>
            </template>
            <template v-else>
              <button :class="treeButtonClass" @click="startRename(selectedNode)">Rename</button>
              <button
                :class="treeButtonClass"
                :title="selectedNode.kind === 'folder'
                  ? 'Delete this folder and the playlists in it. Tracks stay in the collection.'
                  : 'Delete this playlist. Its tracks stay in the collection.'"
                @click="confirmingDelete = selectedNode.id"
              >
                Delete
              </button>
            </template>
          </template>
        </div>
      </div>

      <!-- Tracks -->
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        <div class="flex shrink-0 items-center gap-3 border-b border-border px-4 py-1.5 text-[11px] text-text-muted">
          <span class="w-1.5 shrink-0" />
          <button
            v-for="column in columns"
            :key="column.id"
            class="shrink-0 truncate text-left hover:text-text-secondary"
            :class="[column.class, { 'text-text-primary': sort === column.id }]"
            @click="sortBy(column.id)"
          >
            {{ column.label }}<span v-if="sort === column.id">{{ descending ? ' ↓' : ' ↑' }}</span>
          </button>
        </div>
        <p v-if="listError" class="error-box m-4">{{ listError }}</p>
        <div
          ref="scroller"
          class="min-h-0 flex-1 overflow-y-auto"
          @scroll.passive="onScroll"
          @dragover="overRow($event, total)"
          @drop.prevent="dropOnRow(total)"
        >
          <div class="relative" :style="{ height: `${total * ROW_HEIGHT}px` }">
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
              @dblclick="row.track && (opened = row.track)"
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
                <span class="min-w-0 flex-[3] truncate" :title="row.track.path">{{ row.track.title }}</span>
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
          <p v-if="!total && !listError" class="p-8 text-center text-sm text-text-muted">
            <template v-if="search.trim()">No tracks match.</template>
            <template v-else-if="inPlaylist">Nothing here yet. Drag tracks onto this playlist from the collection.</template>
            <template v-else>No tracks here.</template>
          </p>
        </div>
      </div>

      <CollectionTrackDetail v-if="detailTrack" :track="detailTrack" @close="selection = new Set()" />
    </div>

  </div>
</template>
