import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Job } from '@/types/job';
import type {
  ConsolidatePlan,
  LibraryItem,
  LibraryProgress,
  LibrarySnapshot,
  RelocateReport,
} from '@/types/library';

/** Artwork kept in memory at once; older images are released. */
const MAX_COVERS = 150;
/** Track list thumbnails kept in memory at once; they're a few kilobytes each. */
const MAX_THUMBS = 1000;

/** Collection tracks being tidied: what identifying them found, to review and apply. */
export const useLibraryStore = defineStore('library', () => {
  const items = ref<LibraryItem[]>([]);
  const progress = ref<LibraryProgress>({ phase: 'idle', done: 0, total: 0 });

  const busy = computed(() => progress.value.phase !== 'idle');

  function applySnapshot(snapshot: LibrarySnapshot) {
    items.value = snapshot.items;
    progress.value = snapshot.progress;
  }

  /** Applies a `library-item` event. */
  function upsert(item: LibraryItem) {
    const idx = items.value.findIndex((i) => i.id === item.id);
    if (idx !== -1) items.value[idx] = item;
  }

  async function load() {
    applySnapshot(await invoke<LibrarySnapshot>('get_library'));
  }

  /** Looks collection tracks up and proposes tags and a path for each. */
  const identify = (trackIds: string[]) => invoke('identify_tracks', { trackIds });
  const apply = (ids: string[]) => invoke('apply_library_matches', { ids });
  const cancel = () => invoke('cancel_library_task');
  /** Forgets what identifying found for these items. */
  const dismiss = (ids: string[]) => invoke('dismiss_library_items', { ids });
  const changeMusicDir = (dir: string) => invoke<RelocateReport>('change_music_dir', { dir });
  /** Where the collection's files are: each library folder, and elsewhere. */
  const plan = ref<ConsolidatePlan | null>(null);
  async function loadPlan() {
    plan.value = await invoke<ConsolidatePlan>('consolidate_plan');
  }
  /** Moves the collection's files in the `sources` places into the library folder `target`. */
  const consolidate = (sources: string[], target: string) =>
    invoke<RelocateReport>('consolidate_library', { sources, target });
  /** Downloads a collection track again; it moves to the new file once that succeeds. */
  const redownload = (trackId: string) => invoke<Job>('redownload_track', { trackId });

  /** What MusicBrainz was found to have for each track asked about this session. */
  const genresFound = new Map<string, Promise<string[]>>();

  /**
   * The genres MusicBrainz has for a collection track, the likeliest first.
   * Takes a few seconds the first time: the recording has to be found, and
   * MusicBrainz allows a request a second. Empty when it has none.
   */
  function genres(trackId: string): Promise<string[]> {
    const recordingId = forTrack(trackId)?.proposal?.metadata.mbRecordingId ?? null;
    const key = `${trackId} ${recordingId ?? ''}`;
    let found = genresFound.get(key);
    if (!found) {
      found = invoke<string[]>('musicbrainz_genres', { trackId, recordingId });
      genresFound.set(key, found);
      // A failure isn't an answer: ask again next time.
      found.catch(() => genresFound.delete(key));
    }
    return found;
  }

  const byTrack = computed(() => new Map(items.value.map((i) => [i.trackId, i])));
  /** What identifying found for a collection track, if it has been looked up. */
  const forTrack = (trackId: string) => byTrack.value.get(trackId);
  /** Fixes waiting to be reviewed. */
  const fixes = computed(() => items.value.filter((i) => i.status === 'matched'));

  /**
   * Loads artwork as object URLs by file path, keeping the `max` most
   * recently used and releasing the rest.
   */
  function artLoader(command: string, max: number) {
    const urls = new Map<string, Promise<string | null>>();
    function load(path: string): Promise<string | null> {
      const cached = urls.get(path);
      if (cached) {
        urls.delete(path);
        urls.set(path, cached);
        return cached;
      }
      const loading = invoke<ArrayBuffer>(command, { path })
        .then((bytes) => (bytes.byteLength ? URL.createObjectURL(new Blob([bytes])) : null))
        .catch(() => null);
      urls.set(path, loading);
      while (urls.size > max) {
        const [oldest, url] = urls.entries().next().value!;
        urls.delete(oldest);
        url.then((u) => u && URL.revokeObjectURL(u));
      }
      return loading;
    }
    return { load, has: (path: string) => urls.has(path) };
  }

  const covers = artLoader('library_cover_art', MAX_COVERS);
  const thumbs = artLoader('library_cover_thumb', MAX_THUMBS);
  /** A file's artwork as an object URL, or null when it has none. */
  const coverArt = covers.load;
  /** A file's artwork shrunk for the track list, as an object URL, or null when it has none. */
  const coverThumb = thumbs.load;
  /** Whether a file's thumbnail is loaded, or on its way. */
  const hasCoverThumb = thumbs.has;

  return {
    items,
    progress,
    busy,
    applySnapshot,
    upsert,
    load,
    identify,
    apply,
    cancel,
    dismiss,
    forTrack,
    genres,
    fixes,
    changeMusicDir,
    plan,
    loadPlan,
    consolidate,
    redownload,
    coverArt,
    coverThumb,
    hasCoverThumb,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useLibraryStore, import.meta.hot));
