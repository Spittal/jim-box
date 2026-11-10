import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Job } from '@/types/job';
import type {
  LibraryItem,
  LibraryProgress,
  LibrarySnapshot,
  RelocateReport,
} from '@/types/library';

/** Artwork kept in memory at once; older images are released. */
const MAX_COVERS = 150;

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
  /** Downloads a collection track again; it moves to the new file once that succeeds. */
  const redownload = (trackId: string) => invoke<Job>('redownload_track', { trackId });

  const byTrack = computed(() => new Map(items.value.map((i) => [i.trackId, i])));
  /** What identifying found for a collection track, if it has been looked up. */
  const forTrack = (trackId: string) => byTrack.value.get(trackId);
  /** Fixes waiting to be reviewed. */
  const fixes = computed(() => items.value.filter((i) => i.status === 'matched'));

  // Object URLs for artwork, by file path, oldest first.
  const covers = new Map<string, Promise<string | null>>();

  /** A file's artwork as an object URL, or null when it has none. */
  function coverArt(path: string): Promise<string | null> {
    const cached = covers.get(path);
    if (cached) {
      covers.delete(path);
      covers.set(path, cached);
      return cached;
    }
    const loading = invoke<ArrayBuffer>('library_cover_art', { path })
      .then((bytes) => (bytes.byteLength ? URL.createObjectURL(new Blob([bytes])) : null))
      .catch(() => null);
    covers.set(path, loading);
    while (covers.size > MAX_COVERS) {
      const [oldest, url] = covers.entries().next().value!;
      covers.delete(oldest);
      url.then((u) => u && URL.revokeObjectURL(u));
    }
    return loading;
  }

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
    fixes,
    changeMusicDir,
    redownload,
    coverArt,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useLibraryStore, import.meta.hot));
