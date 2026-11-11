import { acceptHMRUpdate, defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type {
  AnalysisProgress,
  CollectionEdit,
  CollectionState,
  CollectionStatus,
  EditOutcome,
  GridHint,
  ImportReport,
  PlaylistNode,
  Preparation,
  TrackPage,
  TrackPreview,
  TrackQuery,
  Waiting,
} from '@/types/collection';

/** rekordbox's library, read from its master.db by the backend. */
export const useCollectionStore = defineStore('collection', () => {
  const status = ref<CollectionStatus | null>(null);
  const playlists = ref<PlaylistNode[]>([]);
  /** Files waiting to join the collection (rekordbox was open). */
  const waiting = ref<Waiting>({ count: 0, error: null });
  const loading = ref(false);
  const error = ref<string | null>(null);
  /** The analysis queue's current batch; null before any this session. */
  const analysis = ref<AnalysisProgress | null>(null);
  /** Bumped on every (re)load, so track lists know to fetch again. */
  const generation = ref(0);

  function applyState(state: CollectionState) {
    status.value = state.status;
    playlists.value = state.playlists;
    waiting.value = state.waiting;
    generation.value++;
  }

  /**
   * Reads the library from disk, picking up changes made in rekordbox, and
   * adds any files that were waiting for rekordbox to close.
   */
  async function open() {
    loading.value = true;
    error.value = null;
    try {
      applyState(await invoke<CollectionState>('open_collection'));
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Applies a `collection-changed` event: the backend changed the collection. */
  async function changed(now: Waiting) {
    waiting.value = now;
    // Nothing to refresh until the collection has been opened.
    if (!status.value) return;
    try {
      applyState(await invoke<CollectionState>('collection_state'));
    } catch {
      // The next open reports it.
    }
  }

  const tracks = (query: TrackQuery, offset: number, limit: number) =>
    invoke<TrackPage>('collection_tracks', { query, offset, limit });

  /** The id of every track in a list, in its order. */
  const trackIds = (query: TrackQuery) => invoke<string[]>('collection_track_ids', { query });

  /**
   * Makes one change to rekordbox's library. Returns the id of what it
   * created, if anything; on failure sets `error` and returns undefined.
   */
  async function edit(change: CollectionEdit): Promise<string | null | undefined> {
    error.value = null;
    try {
      const outcome = await invoke<EditOutcome>('edit_collection', { edit: change });
      status.value = outcome.status;
      playlists.value = outcome.playlists;
      generation.value++;
      return outcome.createdId;
    } catch (e) {
      error.value = String(e);
      // The backend read the library again; show what is really there.
      generation.value++;
      return undefined;
    }
  }

  /** Marks a track's beat grid as reviewed, or takes that back. */
  async function reviewGrid(trackId: string, reviewed: boolean) {
    await invoke('review_grid', { trackId, reviewed });
    generation.value++;
  }

  /**
   * Adds files, and the music in folders, to the collection (and a playlist).
   * On failure sets `error` and returns undefined.
   */
  async function importPaths(paths: string[], playlistId: string | null): Promise<ImportReport | undefined> {
    error.value = null;
    try {
      const report = await invoke<ImportReport>('import_to_collection', { paths, playlistId });
      applyState(await invoke<CollectionState>('collection_state'));
      return report;
    } catch (e) {
      error.value = String(e);
      return undefined;
    }
  }

  /**
   * Queues tracks for analysis (BPM, beat grid, key, waveforms). Tracks that
   * have one are skipped unless `again`. With null, queues every track in the
   * collection that has none. `hint` is what the user knows of their tempo.
   * Returns how many were queued; on failure sets `error` and returns undefined.
   */
  async function analyse(
    trackIds: string[] | null,
    again: boolean,
    hint: GridHint | null = null,
  ): Promise<number | undefined> {
    error.value = null;
    try {
      return await invoke<number>('analyse_tracks', { trackIds, again, hint });
    } catch (e) {
      error.value = String(e);
      return undefined;
    }
  }

  const cancelAnalysis = () => invoke('cancel_analysis');

  async function loadAnalysis() {
    const progress = await invoke<AnalysisProgress>('analysis_progress');
    if (progress.total) analysis.value = progress;
  }

  /** A track's beat grid and cues, read from disk now. */
  const preparation = (trackId: string) => invoke<Preparation>('track_preparation', { trackId });

  /**
   * A track's detailed waveforms, 150 columns a second, in each form its
   * analysis holds: a byte for the kind (7 three-band, 5 colour, 3 blue),
   * four for the length, then the columns. Empty before analysis.
   */
  const waveform = async (trackId: string) =>
    new Uint8Array(await invoke<ArrayBuffer>('track_waveform', { trackId }));

  /**
   * What the track list shows of these tracks: a small waveform each, in the
   * form `prefer` (7, 5 or 3) where the track has it, and their cues. Tracks
   * no longer in the collection are left out.
   */
  async function previews(trackIds: string[], prefer: number): Promise<TrackPreview[]> {
    type Sent = Omit<TrackPreview, 'data' | 'columns'> & { columns: number[] };
    const sent = await invoke<Sent[]>('track_previews', { trackIds, prefer });
    return sent.map(({ columns, ...preview }) => ({
      ...preview,
      data: Uint8Array.from(columns),
      columns: Math.floor(columns.length / (preview.kind === 7 ? 3 : preview.kind === 5 ? 2 : 1)),
    }));
  }

  return {
    preparation,
    reviewGrid,
    waveform,
    previews,
    status,
    playlists,
    waiting,
    analysis,
    loading,
    error,
    generation,
    open,
    changed,
    tracks,
    trackIds,
    edit,
    importPaths,
    analyse,
    cancelAnalysis,
    loadAnalysis,
  };
});

if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useCollectionStore, import.meta.hot));
