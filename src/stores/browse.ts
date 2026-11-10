import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref, shallowRef, watch } from 'vue';
import { useCollectionStore } from './collection';
import { useJobsStore } from './jobs';
import { isPlaylistLink, usePlaylistStore } from './playlist';
import type { CollectionTrack, TrackQuery, TrackSort, TrackSource } from '@/types/collection';
import type { Job } from '@/types/job';
import type { MissingEntry } from '@/types/playlist';

/** Tracks fetched per request as the list scrolls. */
const PAGE = 200;

/** A track that isn't in the collection yet, listed above the ones that are. */
export interface PendingRow {
  key: string;
  title: string;
  artist: string;
  album: string;
  durationSecs: number;
  /** Its fetch, once one has been started. */
  job: Job | null;
  /** The Spotify playlist track it is, in a playlist that came from Spotify. */
  entry: MissingEntry | null;
}

/** The pending row whose details are showing. */
export type Focus = { kind: 'job'; id: string } | { kind: 'entry'; index: number };

/** What the collection screen is showing: the list, its order, and what's picked in it. */
export const useBrowseStore = defineStore('browse', () => {
  const collection = useCollectionStore();
  const jobs = useJobsStore();
  const playlist = usePlaylistStore();

  // --- Which list -------------------------------------------------------------

  /** The playlist or folder being shown; null for the whole collection. */
  const selected = ref<string | null>(null);
  /** The text in the search bar, which filters the list. */
  const search = ref('');
  const sort = ref<TrackSort>('dateAdded');
  const descending = ref(true);
  /** Show only the tracks still being fetched. */
  const pendingOnly = ref(false);

  const selectedNode = computed(() => collection.playlists.find((p) => p.id === selected.value) ?? null);
  const inPlaylist = computed(() => selectedNode.value?.kind === 'playlist');
  /** Where a fetched track goes besides the collection. */
  const targetPlaylist = computed(() => (inPlaylist.value ? selectedNode.value : null));

  /** A playlist opens in its own order; the collection, newest first. */
  function select(id: string | null) {
    selected.value = id;
    focus.value = null;
    const ordered = id !== null && collection.playlists.find((p) => p.id === id)?.kind === 'playlist';
    sort.value = ordered ? 'trackNo' : 'dateAdded';
    descending.value = !ordered;
  }

  // A playlist deleted (here or in rekordbox) falls back to the collection.
  watch(
    () => collection.playlists,
    () => {
      if (selected.value && !selectedNode.value) select(null);
    },
  );

  function sortBy(column: TrackSort) {
    if (sort.value === column) descending.value = !descending.value;
    else {
      sort.value = column;
      descending.value = column === 'dateAdded' || column === 'rating';
    }
  }

  const query = computed<TrackQuery>(() => {
    const source: TrackSource = selected.value ? { kind: 'playlist', id: selected.value } : { kind: 'collection' };
    // A pasted playlist link is something to bring in, not text to filter by.
    const text = isPlaylistLink(search.value) ? '' : search.value;
    return { source, sort: sort.value, descending: descending.value, query: text };
  });

  // --- Its rows, a page at a time ---------------------------------------------

  const total = ref(0);
  const pages = shallowRef(new Map<number, CollectionTrack[]>());
  const listError = ref<string | null>(null);
  /** Bumped when a different list is shown, so the view scrolls back to its top. */
  const resets = ref(0);
  const requested = new Set<number>();
  /** Identifies the list being shown, so late answers for an old one are dropped. */
  let listVersion = 0;
  /** The rows in view, as last reported by the list. */
  let visible = { first: 0, last: 0 };

  async function fetchPage(page: number) {
    if (requested.has(page)) return;
    requested.add(page);
    const version = listVersion;
    try {
      const result = await collection.tracks(query.value, page * PAGE, PAGE);
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

  /** Fetches the pages holding rows `first` up to `last`. */
  function ensure(first: number, last: number) {
    visible = { first, last };
    if (!collection.status) return;
    const from = Math.floor(first / PAGE);
    const to = Math.floor(Math.max(first, last - 1) / PAGE);
    for (let page = from; page <= to; page++) fetchPage(page);
  }

  /** A different list: start again from the top. */
  function resetList() {
    listVersion++;
    requested.clear();
    pages.value = new Map();
    total.value = 0;
    selection.value = new Set();
    pendingSelection.value = new Set();
    resets.value++;
    ensure(0, 0);
  }

  /** The same list after a change: fetch it again where it is, without a flash. */
  function refreshList() {
    listVersion++;
    requested.clear();
    ensure(visible.first, visible.last);
  }

  function trackAt(index: number): CollectionTrack | null {
    return pages.value.get(Math.floor(index / PAGE))?.[index % PAGE] ?? null;
  }

  /** A track among the rows fetched so far. */
  function loadedTrack(id: string): CollectionTrack | null {
    for (const rows of pages.value.values()) {
      const found = rows.find((r) => r.id === id);
      if (found) return found;
    }
    return null;
  }

  // --- What's picked ----------------------------------------------------------

  const selection = ref(new Set<string>());
  const selectedIds = computed(() => [...selection.value]);
  /** The pending rows picked, by key. Tracks and pending rows aren't picked together. */
  const pendingSelection = ref(new Set<string>());
  const selectedPending = computed(() => pendingRows.value.filter((row) => pendingSelection.value.has(row.key)));
  const focus = ref<Focus | null>(null);
  /** Something to tell the user about the last thing they did. */
  const notice = ref<string | null>(null);

  /** The one selected track, for the detail panel; fresh after each change. */
  const detailTrack = computed(() => (selection.value.size === 1 ? loadedTrack(selectedIds.value[0]) : null));

  function showPending(next: Focus) {
    selection.value = new Set();
    pendingSelection.value = new Set([next.kind === 'entry' ? `entry:${next.index}` : `job:${next.id}`]);
    focus.value = next;
  }

  /** Picks tracks, which lets go of any pending rows. */
  function selectTracks(ids: Iterable<string>) {
    pendingSelection.value = new Set();
    focus.value = null;
    selection.value = new Set(ids);
  }

  /** Picks pending rows, which lets go of any tracks. One row alone shows its details. */
  function selectPending(keys: Set<string>) {
    selection.value = new Set();
    pendingSelection.value = keys;
    const only = keys.size === 1 ? pendingRows.value.find((row) => keys.has(row.key)) : undefined;
    focus.value = !only
      ? null
      : only.entry
        ? { kind: 'entry', index: only.entry.index }
        : { kind: 'job', id: only.job!.id };
  }

  /** Picks everything of the kind already picked: every pending row, or every track of the list. */
  async function selectAll() {
    if (pendingSelection.value.size || pendingOnly.value) {
      selectPending(new Set(pendingRows.value.map((row) => row.key)));
      return;
    }
    try {
      selectTracks(await collection.trackIds(query.value));
    } catch (e) {
      listError.value = String(e);
    }
  }

  function clearSelection() {
    selection.value = new Set();
    pendingSelection.value = new Set();
    focus.value = null;
  }

  // --- Tracks on their way ----------------------------------------------------

  /** The Spotify side of the playlist being shown, if it came from Spotify. */
  const link = computed(() =>
    playlist.linked && playlist.linked.playlistId === selected.value ? playlist.linked : null,
  );

  const pendingRows = computed<PendingRow[]>(() => {
    const rows: PendingRow[] = [];
    const listed = new Set<string>();
    for (const entry of link.value?.missing ?? []) {
      const found = entry.request ? jobs.jobFor(entry.request) : undefined;
      const job = found && found.status !== 'cancelled' ? found : null;
      if (job) listed.add(job.id);
      rows.push({
        key: `entry:${entry.index}`,
        title: entry.track.title,
        artist: entry.track.artists.join(', '),
        album: entry.request?.album ?? '',
        durationSecs: entry.track.durationSecs,
        job,
        entry,
      });
    }
    for (const job of jobs.pending) {
      if (listed.has(job.id)) continue;
      if (selected.value && job.playlistId !== selected.value) continue;
      rows.push({
        key: `job:${job.id}`,
        title: job.metadata?.title ?? job.request.title,
        artist: job.metadata?.artist ?? job.request.artist,
        album: job.metadata?.album ?? job.request.album,
        durationSecs: job.request.durationSecs,
        job,
        entry: null,
      });
    }
    const words = query.value.query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return rows;
    return rows.filter((row) => {
      const text = `${row.title} ${row.artist} ${row.album}`.toLowerCase();
      return words.every((word) => text.includes(word));
    });
  });

  /** What the focused pending row now is: its fetch, or the Spotify track still to match. */
  const focused = computed<{ job: Job | null; entry: MissingEntry | null } | null>(() => {
    const at = focus.value;
    if (!at) return null;
    if (at.kind === 'job') {
      const job = jobs.byId(at.id);
      return job ? { job, entry: null } : null;
    }
    const row = pendingRows.value.find((r) => r.entry?.index === at.index);
    return row ? { job: row.job, entry: row.entry } : null;
  });

  watch(query, resetList, { deep: true });
  watch(() => collection.generation, refreshList);
  // The Spotify side follows the playlist shown, and the collection as tracks land.
  watch(
    [selected, inPlaylist, () => collection.generation],
    () => playlist.show(inPlaylist.value ? selected.value : null),
    { immediate: true },
  );
  watch(
    () => jobs.pending.length,
    (count) => {
      if (!count) pendingOnly.value = false;
    },
  );

  return {
    PAGE,
    selected,
    search,
    sort,
    descending,
    pendingOnly,
    selectedNode,
    inPlaylist,
    targetPlaylist,
    select,
    sortBy,
    total,
    listError,
    resets,
    ensure,
    trackAt,
    loadedTrack,
    selection,
    selectedIds,
    pendingSelection,
    selectedPending,
    selectTracks,
    selectPending,
    selectAll,
    clearSelection,
    focus,
    focused,
    notice,
    detailTrack,
    showPending,
    link,
    pendingRows,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useBrowseStore, import.meta.hot));
