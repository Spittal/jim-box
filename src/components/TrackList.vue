<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue';
import { storeToRefs } from 'pinia';
import CoverArt from '@/components/CoverArt.vue';
import FieldInput from '@/components/FieldInput.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import LibraryCover from '@/components/LibraryCover.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import TrackPreview from '@/components/TrackPreview.vue';
import { focusPane, paneOf } from '@/composables/useDetailKeys';
import { dragging, dropRow, endDrag, startDrag } from '@/composables/useDrag';
import { formatDuration } from '@/composables/useFormat';
import { gridClass, gridLabel, gridSummary } from '@/composables/useGridReport';
import { showContextMenu, type MenuLine } from '@/composables/useContextMenu';
import { useKeyNotation } from '@/composables/useKeyNotation';
import { progressPercent, statusLabel } from '@/composables/useJobStatus';
import { useTrackActions } from '@/composables/useTrackActions';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { PALETTES, chosenPalette } from '@/composables/useWaveform';
import { useBrowseStore, type PendingRow } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { useUiStore } from '@/stores/ui';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { commandKey, revealLabel } from '@/platform';
import type { CollectionTrack, TrackPreview as Preview, TrackSort } from '@/types/collection';

/**
 * The tracks of the list being shown. Tracks still being fetched come first,
 * dimmed, with where their fetch has got to; then the ones in the collection.
 */
const props = defineProps<{
  /** The track in the player, whose row is marked apart from the selected ones. */
  playingId?: string | null;
}>();
const emit = defineEmits<{ open: [track: CollectionTrack] }>();

/** A row's background: the track in the player is tinted, and more so when it's selected too. */
function rowClass(track: CollectionTrack | null | undefined) {
  const picked = !!track && selection.value.has(track.id);
  if (track && track.id === props.playingId) {
    return `shadow-[inset_2px_0_0_var(--color-accent)] ${picked ? 'bg-accent/25' : 'bg-accent/10 hover:bg-accent/15'}`;
  }
  return picked ? 'bg-surface-2' : 'hover:bg-surface-1';
}

const store = useCollectionStore();
const { formatKey } = useKeyNotation();
const { analysis } = storeToRefs(store);
const browse = useBrowseStore();
const { sort, descending, filter, total, listError, selection, selectedIds, pendingRows, pendingSelection, pendingOnly, inPlaylist, selected, detailTrack } =
  storeToRefs(browse);
const jobs = useJobsStore();
const library = useLibraryStore();
const ui = useUiStore();
const actions = useTrackActions();

// Rows are fetched a page at a time as they scroll into view. Only the
// visible slice is in the DOM, so a large library scrolls like a small one.
const ROW_HEIGHT = 28;
const OVERSCAN = 12;
/** The slice moves this many rows at a time, so scrolling renders it once per step, not once per row. */
const STEP = 8;

const scroller = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewportHeight = ref(600);

/** The pending rows sit above the collection's, and scroll away with them. */
const pendingHeight = computed(() => pendingRows.value.length * ROW_HEIGHT);
const listTop = computed(() => Math.max(0, scrollTop.value - pendingHeight.value));

const firstVisible = computed(() => {
  const first = Math.floor(listTop.value / ROW_HEIGHT) - OVERSCAN;
  return Math.max(0, Math.floor(first / STEP) * STEP);
});
const lastVisible = computed(() => {
  const last = Math.ceil((listTop.value + viewportHeight.value) / ROW_HEIGHT) + OVERSCAN;
  return Math.min(total.value, Math.ceil(last / STEP) * STEP);
});

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
  if (scroller.value) scrollTop.value = scroller.value.scrollTop;
}

// Keep the viewport height current as the list appears and the window resizes.
const resizeObserver = new ResizeObserver(([entry]) => {
  viewportHeight.value = entry.contentRect.height;
  onScroll();
});
watch(scroller, (el) => {
  resizeObserver.disconnect();
  if (el) resizeObserver.observe(el);
});
onUnmounted(() => resizeObserver.disconnect());

// --- Selection --------------------------------------------------------------

/**
 * Shift- and ⌘-clicks (Ctrl-clicks on Windows) pick rows; they don't select text. Without this, WebKit
 * stretches a text selection from wherever the caret last was to the click.
 */
function keepTextUnselected(event: MouseEvent) {
  if (!event.shiftKey && !commandKey(event)) return;
  // A field open for editing selects its own text.
  if ((event.target as HTMLElement).closest('input')) return;
  event.preventDefault();
  window.getSelection()?.removeAllRanges();
}

/** A row of the list: a pending one by its key, or a track by where it is and which it is. */
type Spot = { key: string } | { index: number; id: string };

/**
 * Where a shift-click's or shift-arrow's range starts, where the last click or
 * arrow key left off, and what else was picked when the range began: the range
 * can shrink again without losing those.
 */
let anchor: Spot | null = null;
let cursor: Spot | null = null;
let kept = new Set<string>();
/** Counts clicks and key presses, so a pick that had to wait for rows gives way to a later one. */
let picks = 0;

watch(
  () => browse.resets,
  () => {
    anchor = cursor = null;
    kept = new Set();
    picks++;
  },
);

/** Where a track now is: the list may have changed around it since it was picked. */
function indexOf(spot: { index: number; id: string }): number {
  if (browse.trackAt(spot.index)?.id === spot.id) return spot.index;
  return browse.loadedIndex(spot.id) ?? Math.min(spot.index, Math.max(0, total.value - 1));
}

/** Picks one row, or with ⌘ (Ctrl on Windows) adds it to or drops it from what's picked. */
function pick(spot: Spot, toggle = false) {
  picks++;
  const id = 'key' in spot ? spot.key : spot.id;
  const picked = 'key' in spot ? pendingSelection.value : selection.value;
  const next = new Set(toggle ? picked : []);
  if (toggle && next.has(id)) next.delete(id);
  else next.add(id);
  anchor = cursor = spot;
  kept = next;
  if ('key' in spot) browse.selectPending(next);
  else browse.selectTracks(next);
}

/** Picks from the anchor to a row of its kind, in place of the range last picked from it. */
async function extendTo(spot: Spot) {
  const from = anchor;
  if (!from || 'key' in from !== 'key' in spot) return pick(spot);
  cursor = spot;
  const mine = ++picks;
  if ('key' in spot && 'key' in from) {
    const rows = pendingRows.value;
    const a = rows.findIndex((r) => r.key === from.key);
    const b = rows.findIndex((r) => r.key === spot.key);
    if (a === -1 || b === -1) return pick(spot);
    const range = rows.slice(Math.min(a, b), Math.max(a, b) + 1).map((r) => r.key);
    browse.selectPending(new Set([...kept, ...range]));
  } else if ('index' in spot && 'index' in from) {
    const a = indexOf(from);
    const list = browse.resets;
    // Rows that haven't been fetched are in the range too.
    const range = await browse.idsBetween(Math.min(a, spot.index), Math.max(a, spot.index)).catch(() => null);
    if (!range || mine !== picks || list !== browse.resets) return;
    browse.selectTracks(new Set([...kept, ...range]));
  }
}

function clickRow(index: number, track: CollectionTrack, event: MouseEvent) {
  const spot = { index, id: track.id };
  if (event.shiftKey) void extendTo(spot);
  else pick(spot, commandKey(event));
}

/** A row's context menu: what can be done to the selection, which a right-click on a row outside it moves there. */
function rowMenu(index: number, track: CollectionTrack) {
  if (!selection.value.has(track.id)) pick({ index, id: track.id });
  const one = selection.value.size === 1;
  const playlists = store.playlists.filter((p) => p.kind === 'playlist' && p.id !== selected.value);
  const lines: MenuLine[] = [
    { text: 'Open in Player', enabled: one, action: () => emit('open', track) },
    { text: revealLabel, enabled: one, action: () => void revealItemInDir(track.path).catch((e) => (store.error = String(e))) },
    'separator',
    { text: 'Identify', enabled: !library.busy, action: () => void actions.identify() },
    { text: 'Analyse', action: () => void actions.analyse() },
    {
      text: actions.allReviewed() ? 'Not Reviewed' : 'Reviewed',
      enabled: actions.reviewable().length > 0,
      action: () => void actions.review(),
    },
    {
      text: 'Add to Playlist',
      enabled: playlists.length > 0,
      items: playlists.map((p) => ({ text: p.name, action: () => void actions.addToPlaylist(p.id) })),
    },
    'separator',
  ];
  if (inPlaylist.value) lines.push({ text: 'Remove from Playlist', action: () => void actions.removeFromPlaylist() });
  lines.push({ text: 'Delete from Collection…', action: () => void actions.remove() });
  void showContextMenu(lines);
}

// --- Keys ---------------------------------------------------------------------

/** Rows are counted down the whole list: the pending ones, then the collection's. */
const rowCount = computed(() => pendingRows.value.length + (pendingOnly.value ? 0 : total.value));

function positionOf(spot: Spot | null): number | null {
  if (!spot) return null;
  if ('key' in spot) {
    const at = pendingRows.value.findIndex((r) => r.key === spot.key);
    return at === -1 ? null : at;
  }
  return pendingOnly.value || !total.value ? null : pendingRows.value.length + indexOf(spot);
}

/** Scrolls just far enough to show a row. */
function reveal(position: number) {
  const el = scroller.value;
  if (!el) return;
  const top = position * ROW_HEIGHT;
  if (top < el.scrollTop) el.scrollTop = top;
  else if (top + ROW_HEIGHT > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_HEIGHT - el.clientHeight;
}

/**
 * Moves the selection by some rows, or to the list's first or last; with
 * `extend`, stretches it from the anchor instead, as far as the anchor's kind of row goes.
 */
async function move(by: number | 'first' | 'last', extend: boolean) {
  const count = rowCount.value;
  if (!count) return;
  const pending = pendingRows.value.length;
  const from = positionOf(cursor);
  const nothingPicked = !selection.value.size && !pendingSelection.value.size;
  let to: number;
  if (by === 'first') to = 0;
  else if (by === 'last') to = count - 1;
  else if (from === null) {
    // Nowhere to move from: the first row in view going down, the last going up.
    const el = scroller.value;
    const first = Math.ceil((el?.scrollTop ?? 0) / ROW_HEIGHT);
    const last = Math.floor(((el?.scrollTop ?? 0) + (el?.clientHeight ?? 0)) / ROW_HEIGHT) - 1;
    to = by > 0 ? first : last;
  }
  // With nothing picked (after Escape, or the picked tracks were removed), the row left off at is picked again.
  else to = nothingPicked && Math.abs(by) === 1 ? from : from + by;
  to = Math.min(count - 1, Math.max(0, to));
  extend &&= !!anchor && from !== null && !nothingPicked;
  if (extend) to = 'key' in anchor! ? Math.min(to, pending - 1) : Math.max(to, pending);

  reveal(to);
  let spot: Spot;
  if (to < pending) spot = { key: pendingRows.value[to].key };
  else {
    const index = to - pending;
    let id = browse.trackAt(index)?.id;
    if (!id) {
      // A jump to rows that haven't been fetched.
      const mine = ++picks;
      const list = browse.resets;
      [id] = await browse.idsBetween(index, index).catch(() => []);
      if (!id || mine !== picks || list !== browse.resets) return;
    }
    spot = { index, id };
  }
  if (extend) void extendTo(spot);
  else pick(spot);
}

/**
 * Settles the picked track and goes on to the one under it: marks its grid
 * reviewed, picks and loads the next, and puts the keyboard in its sidebar.
 */
async function reviewAndNext() {
  const track = detailTrack.value;
  if (!track) return;
  const mine = picks;
  // What's being typed in a field is saved first, as leaving the field saves it.
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  // Found before the review, which reads the list again.
  const next = await browse.trackAfter(track.id).catch(() => null);
  if (track.analysed !== false && !track.grid?.reviewed) {
    store.error = null;
    void store.reviewGrids([track.id], true).catch((e) => (store.error = String(e)));
  }
  // Unless something else was picked meanwhile.
  if (!next || mine !== picks) return;
  const index = browse.loadedIndex(next.id) ?? (browse.loadedIndex(track.id) ?? -1) + 1;
  reveal(pendingRows.value.length + index);
  pick({ index, id: next.id });
  emit('open', next);
  await nextTick();
  focusPane('detail');
}

function onKey(event: KeyboardEvent) {
  if (event.defaultPrevented || event.altKey || ui.settings) return;
  const target = event.target as HTMLElement | null;
  // From anywhere, a text field included: it's how a track being edited is finished with.
  if (commandKey(event) && event.key.toLowerCase() === 's' && !event.shiftKey) {
    event.preventDefault();
    if (!event.repeat) void reviewAndNext();
    return;
  }
  // Text fields keep their own keys: ⌘A selects their text, arrows move their caret.
  if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
  // With the keyboard in the playlists, the keys are theirs: Delete there isn't to delete tracks.
  if (paneOf(target) === 'playlists') return;
  const command = commandKey(event);
  const page = Math.max(1, Math.floor(viewportHeight.value / ROW_HEIGHT) - 1);
  const picked = selection.value.size || pendingSelection.value.size;

  if (command && !event.shiftKey && event.key.toLowerCase() === 'a') void browse.selectAll();
  else if (event.key === 'ArrowDown') void move(command ? 'last' : 1, event.shiftKey);
  else if (event.key === 'ArrowUp') void move(command ? 'first' : -1, event.shiftKey);
  else if (event.key === 'PageDown') void move(page, event.shiftKey);
  else if (event.key === 'PageUp') void move(-page, event.shiftKey);
  else if (event.key === 'Home') void move('first', event.shiftKey);
  else if (event.key === 'End') void move('last', event.shiftKey);
  else if (event.key === 'Escape' && picked) browse.clearSelection();
  else if (event.key === 'Enter' && detailTrack.value && !target?.closest('button, a')) {
    // Loaded to be worked on: the keyboard goes to its sidebar.
    emit('open', detailTrack.value);
    focusPane('detail');
  }
  else if ((event.key === 'Delete' || event.key === 'Backspace') && selection.value.size) {
    // With ⌘, out of the collection and its file to the Trash, wherever it's shown.
    // Else out of a playlist at once, as its button does; out of the collection once that's been asked.
    if (command) void actions.remove(true);
    else if (inPlaylist.value) void actions.removeFromPlaylist();
    else void actions.remove();
  } else return;
  event.preventDefault();
}
onMounted(() => window.addEventListener('keydown', onKey));
onUnmounted(() => window.removeEventListener('keydown', onKey));

/** Clicking the current rating clears it, as in rekordbox. */
function rate(track: CollectionTrack, stars: number) {
  const next = track.rating === stars ? 0 : stars;
  // Shown at once: saving takes a moment, and a star should answer the click.
  browse.patchTrack(track.id, { rating: next });
  store.edit({ kind: 'setRating', trackId: track.id, stars: next });
}

// --- Editing in place ---------------------------------------------------------

/** The fields a row can be edited in. */
type CellField = 'title' | 'artist' | 'album' | 'genre';

/** The field being edited in its row. */
const editing = ref<{ id: string; field: CellField } | null>(null);

/**
 * A double-click opens a track in the player. On the track already there, it
 * opens the field under it for editing instead.
 */
function doubleClickRow(event: MouseEvent, track: CollectionTrack) {
  // By where the click was across the row: an empty field has no text to land on.
  const cells = (event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[data-field]');
  const cell = [...cells].find((el) => {
    const { left, right } = el.getBoundingClientRect();
    return event.clientX >= left && event.clientX <= right;
  });
  const field = cell?.dataset.field as CellField | undefined;
  if (field && track.id === props.playingId) editing.value = { id: track.id, field };
  else emit('open', track);
}

/** Saves what was typed into the open field, if it changed. */
function commitCell(value: string) {
  const cell = editing.value;
  editing.value = null;
  if (!cell) return;
  const track = browse.loadedTrack(cell.id);
  if (!track || value === track[cell.field]) return;
  // Shown at once: saving takes a moment.
  browse.patchTrack(cell.id, { [cell.field]: value });
  store.edit({ kind: 'setField', trackId: cell.id, field: cell.field, value });
}

// A different list has nothing of the field that was open.
watch(
  () => browse.resets,
  () => (editing.value = null),
);

// --- Tracks on their way ----------------------------------------------------

/** Picks pending rows as tracks are picked: ⌘ adds or drops one, shift takes a range. */
function clickPending(row: PendingRow, event?: MouseEvent) {
  const spot = { key: row.key };
  if (event?.shiftKey) void extendTo(spot);
  else pick(spot, !!event && commandKey(event));
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

// --- Work under way on a track --------------------------------------------------

/** What is being done to a collection track, shown along its row. */
interface Activity {
  label: string;
  /** How far along it is, where that's known; null for a bar that just moves. */
  percent: number | null;
  /** Waiting its turn: named, without a bar. */
  waiting?: boolean;
}

const analysing = computed(() => new Set(analysis.value?.running));
const awaitingAnalysis = computed(() => new Set(analysis.value?.queued));

/** A fetch replacing the file, then tagging and identifying, then analysis: one at a time, the busiest first. */
function activityOf(track: CollectionTrack): Activity | null {
  const job = jobs.replacing.get(track.path);
  if (job) {
    const percent = progressPercent(job);
    return { label: percent === null ? `New copy: ${statusLabel(job)}` : statusLabel(job), percent };
  }
  const item = library.forTrack(track.id);
  if (item?.status === 'applying') return { label: 'Applying tags', percent: null };
  if (analysing.value.has(track.id)) return { label: 'Analysing', percent: null };
  if (item?.status === 'pending' && library.busy) return { label: 'Identifying', percent: null };
  if (awaitingAnalysis.value.has(track.id)) return { label: 'Waiting to analyse', percent: null, waiting: true };
  return null;
}

/** The rows in view with work under way, by track id. */
const activities = computed(() => {
  const found = new Map<string, Activity>();
  for (const { track } of visibleRows.value) {
    const activity = track && activityOf(track);
    if (activity) found.set(track.id, activity);
  }
  return found;
});

// --- Reordering a playlist ---------------------------------------------------

/** Tracks can be dragged into a new order only in the playlist's own order. */
const reorderable = computed(
  () => inPlaylist.value && sort.value === 'trackNo' && !descending.value && !filter.value && !pendingOnly.value,
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

/** A column: something the list can be sorted by, or the artwork, or the waveform. */
type ColumnId = TrackSort | 'art' | 'preview';

/** The list can be sorted by every column but the pictures. */
const sortable = (id: ColumnId): id is TrackSort => id !== 'art' && id !== 'preview';

interface Column {
  id: ColumnId;
  label: string;
  /** Its name in the columns menu, where the header's is too short to say. */
  name?: string;
  /** Its width in pixels until it's dragged to another; without one it shares the room left over. */
  width?: number;
  /** How much of the room left over it takes, against the others sharing it. */
  grow?: number;
  /** How its header and cells are set, where not to the left. */
  class?: string;
  /** Shown only for the whole collection. */
  collectionOnly?: boolean;
}

const COLUMNS: Column[] = [
  { id: 'art', label: '', name: 'Artwork', width: 16 },
  { id: 'trackNo', label: '#', name: 'Number', width: 40, class: 'text-center' },
  { id: 'preview', label: 'Preview', name: 'Waveform and cues', width: 160, class: 'text-center' },
  { id: 'title', label: 'Title', grow: 3 },
  { id: 'artist', label: 'Artist', grow: 2 },
  { id: 'album', label: 'Album', grow: 2 },
  { id: 'genre', label: 'Genre', grow: 1 },
  { id: 'playlists', label: 'Playlists', grow: 2, collectionOnly: true },
  { id: 'bpm', label: 'BPM', width: 56, class: 'text-center' },
  { id: 'confidence', label: 'Grid', name: 'Grid confidence', width: 44, class: 'text-center' },
  { id: 'key', label: 'Key', width: 40, class: 'text-center' },
  { id: 'duration', label: 'Time', width: 48, class: 'text-center' },
  { id: 'rating', label: 'Rating', width: 64, class: 'text-center' },
  { id: 'dateAdded', label: 'Added', width: 96, class: 'text-center' },
  { id: 'path', label: 'File path', grow: 3 },
];

/** The columns' order, which are hidden, and the widths they've been dragged to, kept between runs. */
const COLUMNS_KEY = 'tracks.columns';
function savedColumns(): { order: ColumnId[]; hidden: ColumnId[]; widths: Partial<Record<ColumnId, number>> } {
  try {
    const saved = JSON.parse(localStorage.getItem(COLUMNS_KEY) ?? '{}');
    const known = (ids: unknown) =>
      Array.isArray(ids) ? ids.filter((id): id is ColumnId => COLUMNS.some((c) => c.id === id)) : [];
    const order = known(saved.order);
    // A column added since the order was saved goes where it is in the list above.
    COLUMNS.forEach((column, at) => {
      if (!order.includes(column.id)) order.splice(Math.min(at, order.length), 0, column.id);
    });
    const widths: Partial<Record<ColumnId, number>> = {};
    for (const { id } of COLUMNS) {
      const width = saved.widths?.[id];
      if (typeof width === 'number' && width > 0) widths[id] = width;
    }
    return { order, hidden: known(saved.hidden), widths };
  } catch {
    return { order: COLUMNS.map((c) => c.id), hidden: [], widths: {} };
  }
}
const columnOrder = ref(savedColumns().order);
const hiddenColumns = ref(new Set(savedColumns().hidden));
const columnWidths = ref(savedColumns().widths);
watch([columnOrder, hiddenColumns, columnWidths], () =>
  localStorage.setItem(
    COLUMNS_KEY,
    JSON.stringify({ order: columnOrder.value, hidden: [...hiddenColumns.value], widths: columnWidths.value }),
  ),
);

/** Every column this list can show, in the order chosen. */
const available = computed(() =>
  columnOrder.value
    .map((id) => COLUMNS.find((c) => c.id === id)!)
    .filter((column) => !column.collectionOnly || selected.value === null),
);
const columns = computed(() => available.value.filter((column) => !hiddenColumns.value.has(column.id)));

const columnsMenu = ref(false);

function toggleColumn(id: ColumnId) {
  const hidden = new Set(hiddenColumns.value);
  if (!hidden.delete(id)) hidden.add(id);
  hiddenColumns.value = hidden;
}

function resetColumns() {
  columnOrder.value = COLUMNS.map((c) => c.id);
  hiddenColumns.value = new Set();
  columnWidths.value = {};
}

// --- Column widths ------------------------------------------------------------

/** The gap between columns, and the room either side of them: the rows' `gap-3`, `pl-4` and `pr-7`. */
const COLUMN_GAP = 12;
const ROW_PADDING = 16 + 28;
/** The narrowest a column goes, and the least one sharing the room left over is squeezed to. */
const MIN_WIDTH = 24;
const MIN_SHARED_WIDTH = 48;
/** The artwork is square, so it's no wider than a row is tall. */
const ART_WIDTHS = [12, ROW_HEIGHT - 4] as const;

const clampWidth = (id: ColumnId, width: number) =>
  id === 'art'
    ? Math.min(ART_WIDTHS[1], Math.max(ART_WIDTHS[0], Math.round(width)))
    : Math.max(MIN_WIDTH, Math.round(width));

/** A column's width in pixels, or undefined where it shares the room left over. */
const widthOf = (column: Column) => {
  const width = columnWidths.value[column.id] ?? column.width;
  return width === undefined ? undefined : clampWidth(column.id, width);
};

/** How each column's header and cells are sized. */
const cellStyle = computed(() => {
  const styles = {} as Record<ColumnId, Record<string, string>>;
  for (const column of COLUMNS) {
    const width = widthOf(column);
    if (width === undefined) styles[column.id] = { flex: `${column.grow ?? 1} 1 0%`, minWidth: `${MIN_SHARED_WIDTH}px` };
    else if (column.id === 'art') styles[column.id] = { flex: `0 0 ${width}px`, width: `${width}px`, height: `${width}px` };
    else styles[column.id] = { flex: `0 0 ${width}px`, width: `${width}px` };
  }
  return styles;
});

/** The least room the columns need; in less, the list scrolls sideways. */
const minListWidth = computed(
  () =>
    ROW_PADDING +
    columns.value.reduce((sum, column) => sum + COLUMN_GAP + (widthOf(column) ?? MIN_SHARED_WIDTH), 0) -
    COLUMN_GAP,
);

/** The column whose edge is being dragged, where the drag began, and how wide the column was then. */
const resizing = ref<{ id: ColumnId; startX: number; startWidth: number } | null>(null);

function startResize(event: PointerEvent, id: ColumnId) {
  if (event.button !== 0) return;
  const handle = event.currentTarget as HTMLElement;
  const header = handle.parentElement;
  if (!header) return;
  event.preventDefault();
  handle.setPointerCapture(event.pointerId);
  resizing.value = { id, startX: event.clientX, startWidth: header.getBoundingClientRect().width };
}

function resize(event: PointerEvent) {
  const drag = resizing.value;
  if (!drag) return;
  const width = clampWidth(drag.id, drag.startWidth + event.clientX - drag.startX);
  if (columnWidths.value[drag.id] !== width) columnWidths.value = { ...columnWidths.value, [drag.id]: width };
}

function endResize() {
  resizing.value = null;
}

/** Puts a column back to the width it came with. */
function resetWidth(id: ColumnId) {
  const { [id]: _, ...rest } = columnWidths.value;
  columnWidths.value = rest;
}

/** The column header being dragged to a new place, and the one it is over. */
const movingColumn = ref<ColumnId | null>(null);
const columnTarget = ref<ColumnId | null>(null);

function dragColumn(event: DragEvent, id: ColumnId) {
  movingColumn.value = id;
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', id);
  }
}

function overColumn(event: DragEvent, id: ColumnId) {
  if (!movingColumn.value) return;
  event.preventDefault();
  columnTarget.value = id;
}

/** Puts the dragged column where `id` is, which moves along. */
function dropColumn(id: ColumnId) {
  const moving = movingColumn.value;
  endColumnDrag();
  if (!moving || moving === id) return;
  const order = columnOrder.value.filter((c) => c !== moving);
  const from = columnOrder.value.indexOf(moving);
  const to = columnOrder.value.indexOf(id);
  // Dragged rightwards it lands after the column it's dropped on; leftwards, before.
  order.splice(order.indexOf(id) + (from < to ? 1 : 0), 0, moving);
  columnOrder.value = order;
}

function endColumnDrag() {
  movingColumn.value = null;
  columnTarget.value = null;
}

// --- Waveform previews --------------------------------------------------------

/** How long rows must stay in view before their previews are fetched, so rows scrolled past fetch nothing. */
const PREVIEW_SETTLE_MS = 120;
/** Previews kept at once; they're about a kilobyte each. */
const MAX_PREVIEWS = 2000;

/** The waveform and cues of the tracks that have been in view, by track id. */
const previews = shallowRef(new Map<string, Preview>());
/** What each was read as: the collection it was read from, and in which style. */
const previewRead = new Map<string, string>();
const previewShown = computed(() => columns.value.some((column) => column.id === 'preview'));
const previewKind = computed(() => PALETTES.find((p) => p.id === chosenPalette.value)?.kind ?? 7);

/**
 * Reads the previews of the rows in view that haven't been read since the
 * collection last changed: a cue set in the player, or an analysis finishing,
 * shows here too. What was read before stays up until its replacement arrives.
 */
async function loadPreviews() {
  if (!previewShown.value) return;
  const read = `${store.generation}/${previewKind.value}`;
  const inView = visibleRows.value.flatMap((row) => (row.track ? [row.track.id] : []));
  const ids = inView.filter((id) => previewRead.get(id) !== read);
  if (!ids.length) return;
  for (const id of ids) previewRead.set(id, read);
  try {
    const found = await store.previews(ids, previewKind.value);
    const next = new Map(previews.value);
    for (const preview of found) {
      // Unless it has been asked for again since, for a newer collection.
      if (previewRead.get(preview.trackId) === read) next.set(preview.trackId, preview);
    }
    if (next.size > MAX_PREVIEWS) {
      const keep = new Set(inView);
      for (const id of next.keys()) {
        if (keep.has(id)) continue;
        next.delete(id);
        previewRead.delete(id);
      }
    }
    previews.value = next;
  } catch {
    // Left for the next time they're in view.
    for (const id of ids) if (previewRead.get(id) === read) previewRead.delete(id);
  }
}

let previewTimer = 0;
watch(
  [visibleRows, () => store.generation, previewKind, previewShown],
  () => {
    clearTimeout(previewTimer);
    previewTimer = window.setTimeout(loadPreviews, PREVIEW_SETTLE_MS);
  },
  { immediate: true },
);
onUnmounted(() => clearTimeout(previewTimer));

/** A field open in its row: the row's own text, on a ground that spreads past it without moving anything. */
const editClass =
  'min-w-0 rounded-sm bg-surface-0 text-text-primary outline-none select-text shadow-[0_0_0_3px_var(--color-surface-0),0_0_0_4px_var(--color-accent)]';

const smallButtonClass =
  'shrink-0 rounded bg-surface-2 px-2 py-0.5 text-[11px] text-text-primary transition-colors hover:bg-surface-3';
</script>

<template>
  <div data-pane="list" class="flex min-h-0 min-w-0 flex-1 flex-col overflow-x-auto">
  <div class="flex min-h-0 flex-1 flex-col" :style="{ minWidth: `${minListWidth}px` }">
    <div
      class="relative flex shrink-0 items-center gap-3 border-b border-border py-1.5 pr-7 pl-4 text-[11px] text-text-muted"
      :class="{ 'cursor-col-resize': resizing }"
      @contextmenu.prevent="columnsMenu = true"
    >
      <div
        v-for="column in columns"
        :key="column.id"
        class="relative flex"
        :style="{ ...cellStyle[column.id], height: 'auto' }"
      >
      <button
        class="min-w-0 flex-1 truncate hover:text-text-secondary"
        :class="[
          column.class ?? 'text-left',
          { 'text-text-primary': sort === column.id, 'opacity-40': movingColumn === column.id },
          columnTarget === column.id && movingColumn !== column.id ? 'text-accent' : '',
        ]"
        :title="sortable(column.id)
          ? 'Click to sort by this. Drag to move the column.'
          : `${column.name}. Drag to move the column.`"
        draggable="true"
        @click="sortable(column.id) && browse.sortBy(column.id)"
        @dragstart="dragColumn($event, column.id)"
        @dragover="overColumn($event, column.id)"
        @drop.prevent="dropColumn(column.id)"
        @dragend="endColumnDrag"
      >
        {{ column.label || '\u00a0' }}<span v-if="sort === column.id">{{ descending ? ' ↓' : ' ↑' }}</span>
      </button>
        <!-- The column's right edge, in the gap after it: drag to resize. -->
        <span
          class="group absolute -top-1.5 -right-2.5 -bottom-1.5 z-[1] flex w-2 cursor-col-resize justify-center"
          title="Drag to resize the column. Double-click for its usual width."
          @pointerdown="startResize($event, column.id)"
          @pointermove="resize"
          @pointerup="endResize"
          @pointercancel="endResize"
          @dblclick.stop="resetWidth(column.id)"
          @click.stop
        >
          <span
            class="w-px group-hover:bg-text-muted"
            :class="resizing?.id === column.id ? 'bg-accent' : 'bg-border'"
          />
        </span>
      </div>
      <button
        class="absolute top-1/2 right-0.5 -translate-y-1/2 rounded px-1 leading-none hover:bg-surface-2 hover:text-text-primary"
        title="Choose columns"
        @click="columnsMenu = !columnsMenu"
      >
        ⋮
      </button>
      <template v-if="columnsMenu">
        <div class="fixed inset-0 z-10" @click="columnsMenu = false" @contextmenu.prevent.stop="columnsMenu = false" />
        <div
          class="absolute top-full right-1 z-20 mt-1 flex w-40 flex-col rounded border border-border bg-surface-1 py-1 text-xs text-text-primary shadow-lg"
          @contextmenu.prevent.stop
        >
          <label
            v-for="column in available"
            :key="column.id"
            class="flex cursor-default items-center gap-2 px-3 py-1 hover:bg-surface-2"
          >
            <input
              type="checkbox"
              :checked="!hiddenColumns.has(column.id)"
              :disabled="columns.length === 1 && !hiddenColumns.has(column.id)"
              @change="toggleColumn(column.id)"
            />
            {{ column.name ?? column.label }}
          </label>
          <button
            class="mt-1 border-t border-border px-3 pt-1.5 pb-0.5 text-left text-text-muted hover:text-text-primary"
            @click="resetColumns"
          >
            Reset columns
          </button>
        </div>
      </template>
    </div>
    <p v-if="listError" class="error-box m-4">{{ listError }}</p>
    <div
      ref="scroller"
      data-track-list
      tabindex="-1"
      class="min-h-0 flex-1 overflow-y-auto outline-none select-none"
      @scroll.passive="onScroll"
      @mousedown="keepTextUnselected"
      @dragover="overRow($event, total)"
      @drop.prevent="dropOnRow(total)"
    >
      <!-- Tracks on their way: what is known of them under its column, then where their fetch has got to. -->
      <div
        v-for="row in pendingRows"
        :key="row.key"
        class="flex cursor-default items-center gap-3 pr-7 pl-4 text-xs text-text-secondary"
        :class="pendingSelection.has(row.key) ? 'bg-surface-2' : 'hover:bg-surface-1'"
        :style="{ height: `${ROW_HEIGHT}px` }"
        @click="clickPending(row, $event)"
      >
        <template v-for="column in columns" :key="column.id">
          <CoverArt
            v-if="column.id === 'art'"
            :src="row.job?.request.coverUrl ?? row.entry?.request?.coverUrl ?? null"
            :style="cellStyle.art"
          />
          <span
            v-else-if="column.id === 'trackNo'"
            class="truncate font-mono text-text-muted"
            :class="column.class" :style="cellStyle[column.id]"
            title="Not in the collection yet"
          >↓</span>
          <span v-else-if="column.id === 'preview'" :class="column.class" :style="cellStyle[column.id]" />
          <span v-else-if="column.id === 'title'" class="truncate" :class="column.class" :style="cellStyle[column.id]">{{ row.title }}</span>
          <span v-else-if="column.id === 'artist'" class="truncate text-text-muted" :class="column.class" :style="cellStyle[column.id]">{{ row.artist }}</span>
          <span v-else-if="column.id === 'album'" class="truncate text-text-muted" :class="column.class" :style="cellStyle[column.id]">{{ row.album }}</span>
        </template>
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
          class="absolute right-0 left-0 flex cursor-default items-center gap-3 pr-7 pl-4 text-xs"
          :class="[
            rowClass(row.track),
            dropRow === row.index ? 'border-t border-accent' : '',
          ]"
          :style="{ top: `${row.index * ROW_HEIGHT}px`, height: `${ROW_HEIGHT}px` }"
          :draggable="!!row.track && editing?.id !== row.track.id"
          @click="row.track && clickRow(row.index, row.track, $event)"
          @dblclick="row.track && doubleClickRow($event, row.track)"
          @contextmenu.prevent="row.track && rowMenu(row.index, row.track)"
          @dragstart="row.track && dragTrack($event, row.track)"
          @dragend="endDrag"
          @dragover.stop="overRow($event, row.index)"
          @drop.prevent.stop="dropOnRow(row.index)"
        >
          <template v-if="row.track">
            <!-- The track's colour, in the row's left margin. -->
            <span
              v-if="TRACK_COLORS[row.track.color]?.hex"
              class="absolute top-1/2 left-1.5 size-1.5 -translate-y-1/2 rounded-full"
              :style="{ backgroundColor: TRACK_COLORS[row.track.color].hex }"
            />
            <template v-for="column in columns" :key="column.id">
            <!-- The field double-clicked, open for editing in place of its text. -->
            <FieldInput
              v-if="editing?.id === row.track.id && editing.field === column.id"
              :field="editing.field"
              :value="row.track[editing.field]"
              autofocus
              commit-on-remove
              :class="[editClass, column.class]"
              :style="cellStyle[column.id]"
              @commit="commitCell"
              @cancel="editing = null"
              @click.stop
              @dblclick.stop
              @contextmenu.stop
            />
            <LibraryCover v-else-if="column.id === 'art'" :path="row.track.path" thumb :style="cellStyle.art" />
            <span v-else-if="column.id === 'trackNo'" class="truncate font-mono text-text-muted" :class="column.class" :style="cellStyle[column.id]">{{ row.track.position }}</span>
            <TrackPreview
              v-else-if="column.id === 'preview'"
              :preview="previews.get(row.track.id) ?? null"
              :class="column.class" :style="cellStyle[column.id]"
            />
            <span v-else-if="column.id === 'title'" data-field="title" class="flex items-center gap-1.5 overflow-hidden" :class="column.class" :style="cellStyle[column.id]">
              <span
                class="truncate"
                :class="row.track.id === playingId ? 'font-medium text-accent-hover' : ''"
                :title="row.track.path"
              >{{ row.track.title }}</span>
              <span
                v-if="library.forTrack(row.track.id)?.status === 'matched'"
                class="shrink-0 rounded bg-accent/15 px-1.5 font-mono text-[10px] leading-4 text-accent-hover"
                title="Identifying found tags or a file name to fix. Select the track to review it."
              >fix</span>
              <span
                v-if="activities.has(row.track.id)"
                class="flex shrink-0 items-center gap-1.5 text-[11px]"
                :class="activities.get(row.track.id)!.waiting ? 'text-text-muted' : 'text-accent-hover'"
              >
                <span
                  class="size-1.5 rounded-full"
                  :class="activities.get(row.track.id)!.waiting ? 'bg-status-idle' : 'animate-pulse bg-accent'"
                />
                {{ activities.get(row.track.id)!.label }}
              </span>
            </span>
            <span v-else-if="column.id === 'artist'" data-field="artist" class="truncate text-text-secondary" :class="column.class" :style="cellStyle[column.id]">{{ row.track.artist }}</span>
            <span v-else-if="column.id === 'album'" data-field="album" class="truncate text-text-secondary" :class="column.class" :style="cellStyle[column.id]">{{ row.track.album }}</span>
            <span v-else-if="column.id === 'genre'" data-field="genre" class="truncate text-text-muted" :class="column.class" :style="cellStyle[column.id]">{{ row.track.genre }}</span>
            <span
              v-else-if="column.id === 'playlists'"
              class="truncate text-text-muted"
              :class="column.class" :style="cellStyle[column.id]"
              :title="row.track.playlists.join('\n')"
            >{{ row.track.playlists.join(', ') }}</span>
            <span v-else-if="column.id === 'bpm'" class="truncate font-mono" :class="column.class" :style="cellStyle[column.id]">
              <span v-if="analysis?.running.includes(row.track.id)" class="text-text-muted" title="Being analysed">…</span>
              <template v-else>{{ row.track.bpm?.toFixed(1) ?? '' }}</template>
            </span>
            <span
              v-else-if="column.id === 'confidence'"
              class="truncate font-mono"
              :class="[column.class, row.track.grid ? gridClass(row.track.grid) : '']"
              :style="cellStyle[column.id]"
              :title="row.track.grid ? gridSummary(row.track.grid) : undefined"
            >
              <template v-if="row.track.grid">{{ gridLabel(row.track.grid) }}</template>
            </span>
            <span v-else-if="column.id === 'key'" class="truncate font-mono text-text-secondary" :class="column.class" :style="cellStyle[column.id]">{{ formatKey(row.track.key) }}</span>
            <span v-else-if="column.id === 'duration'" class="truncate font-mono text-text-muted" :class="column.class" :style="cellStyle[column.id]">
              {{ row.track.durationSecs ? formatDuration(row.track.durationSecs) : '' }}
            </span>
            <span
              v-else-if="column.id === 'rating'"
              class="flex justify-center overflow-hidden text-[11px] leading-none"
              :style="cellStyle[column.id]"
              @click.stop
              @dblclick.stop
            >
              <button
                tabindex="-1"
                v-for="star in 5"
                :key="star"
                :class="star <= row.track.rating ? 'text-text-secondary hover:text-text-primary' : 'text-text-muted/50 hover:text-text-secondary'"
                :title="`${star} ${star === 1 ? 'star' : 'stars'}`"
                @click="rate(row.track, star)"
              >{{ star <= row.track.rating ? '★' : '☆' }}</button>
            </span>
            <span v-else-if="column.id === 'dateAdded'" class="truncate font-mono text-text-muted" :class="column.class" :style="cellStyle[column.id]">{{ row.track.dateAdded.slice(0, 10) }}</span>
            <span
              v-else-if="column.id === 'path'"
              class="truncate text-text-muted"
              :class="column.class" :style="cellStyle[column.id]"
              :title="row.track.path"
            >{{ row.track.path }}</span>
            </template>
            <!-- Work under way on the track, along the row's bottom edge. -->
            <span
              v-if="activities.has(row.track.id) && !activities.get(row.track.id)!.waiting"
              class="pointer-events-none absolute right-0 bottom-0 left-0 h-0.5 overflow-hidden bg-accent/15"
            >
              <span
                v-if="activities.get(row.track.id)!.percent !== null"
                class="block h-full bg-accent transition-[width] duration-500"
                :style="{ width: `${activities.get(row.track.id)!.percent}%` }"
              />
              <span v-else class="block h-full w-1/5 animate-row-busy bg-accent motion-reduce:w-full motion-reduce:animate-none motion-reduce:opacity-50" />
            </span>
          </template>
        </div>
      </div>
      <p
        v-if="!listError && !pendingRows.length && (pendingOnly || !total)"
        class="p-8 text-center text-sm text-text-muted"
      >
        <template v-if="filter">
          Nothing here matches. Press Return to look for “{{ filter }}” online.
        </template>
        <template v-else-if="pendingOnly">Nothing is being fetched.</template>
        <template v-else-if="inPlaylist">
          Nothing here yet. Drag tracks onto this playlist, or search below and add what you find.
        </template>
        <template v-else>No tracks here.</template>
      </p>
    </div>
  </div>
  </div>
</template>
