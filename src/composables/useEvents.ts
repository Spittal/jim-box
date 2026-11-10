import { onUnmounted } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useCollectionStore } from '@/stores/collection';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { usePlaylistStore } from '@/stores/playlist';
import type { AnalysisProgress, Waiting } from '@/types/collection';
import type { DaemonStatus } from '@/types/daemon';
import type { Job } from '@/types/job';
import type { LibraryItem, LibraryProgress, LibrarySnapshot } from '@/types/library';
import type { PlaylistProgress } from '@/types/playlist';

/** Subscribes stores to backend events for the lifetime of the calling component. */
export function useEvents() {
  const daemon = useDaemonStore();
  const jobs = useJobsStore();
  const library = useLibraryStore();
  const playlist = usePlaylistStore();
  const collection = useCollectionStore();
  const unlisteners: Promise<UnlistenFn>[] = [
    listen<DaemonStatus>('daemon-status', (e) => {
      daemon.status = e.payload;
    }),
    listen<Job>('job-updated', (e) => jobs.upsert(e.payload)),
    listen<string>('job-removed', (e) => jobs.drop(e.payload)),
    listen<LibrarySnapshot>('library-reset', (e) => library.applySnapshot(e.payload)),
    listen<LibraryItem>('library-item', (e) => library.upsert(e.payload)),
    listen<LibraryProgress>('library-progress', (e) => {
      library.progress = e.payload;
    }),
    listen<Waiting>('collection-changed', (e) => collection.changed(e.payload)),
    listen<AnalysisProgress>('analysis-progress', (e) => {
      collection.analysis = e.payload;
    }),
    listen<PlaylistProgress>('playlist-progress', (e) => {
      playlist.progress = e.payload;
    }),
  ];

  onUnmounted(() => {
    unlisteners.forEach((p) => p.then((unlisten) => unlisten()));
  });
}
