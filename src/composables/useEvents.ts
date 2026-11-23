import { onUnmounted } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useCollectionStore } from '@/stores/collection';
import { useSoulseekStore } from '@/stores/soulseek';
import { useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { useLibrariesStore } from '@/stores/libraries';
import { usePlaylistStore } from '@/stores/playlist';
import { useUsbStore } from '@/stores/usb';
import type { AnalysisEvent, Waiting } from '@/types/collection';
import type { SoulseekStatus } from '@/types/soulseek';
import type { Job } from '@/types/job';
import type { LibraryItem, LibraryProgress, LibrarySnapshot } from '@/types/library';
import type { PlaylistProgress } from '@/types/playlist';
import type { UsbProgress } from '@/types/usb';

/** Subscribes stores to backend events for the lifetime of the calling component. */
export function useEvents() {
  const soulseek = useSoulseekStore();
  const jobs = useJobsStore();
  const library = useLibraryStore();
  const playlist = usePlaylistStore();
  const collection = useCollectionStore();
  const usb = useUsbStore();
  const libraries = useLibrariesStore();
  const unlisteners: Promise<UnlistenFn>[] = [
    listen<SoulseekStatus>('soulseek-status', (e) => {
      soulseek.status = e.payload;
    }),
    listen<Job>('job-updated', (e) => jobs.upsert(e.payload)),
    listen<{ id: string; progress: Job['progress'] }>('job-progress', (e) => jobs.progressed(e.payload.id, e.payload.progress)),
    listen<string>('job-removed', (e) => jobs.drop(e.payload)),
    listen<LibrarySnapshot>('library-reset', (e) => library.applySnapshot(e.payload)),
    listen<LibraryItem>('library-item', (e) => library.upsert(e.payload)),
    listen<LibraryProgress>('library-progress', (e) => {
      library.progress = e.payload;
    }),
    listen<Waiting>('collection-changed', (e) => collection.changed(e.payload)),
    listen<AnalysisEvent>('analysis-progress', (e) => collection.analysisMoved(e.payload)),
    listen<PlaylistProgress>('playlist-progress', (e) => {
      playlist.progress = e.payload;
    }),
    listen<string>('playlist-synced', (e) => playlist.synced(e.payload)),
    // Another library opened, closed or changed (a stick pulled out closes its own).
    listen('libraries-changed', () => libraries.refresh()),
    listen('volumes-changed', () => {
      void libraries.refreshPlugged();
      if (usb.devices.length || usb.looking) void usb.load();
    }),
    listen<UsbProgress>('usb-progress', (e) => {
      // One that arrives after the export is over says nothing new.
      if (usb.exporting) usb.progress = e.payload;
    }),
  ];

  onUnmounted(() => {
    unlisteners.forEach((p) => p.then((unlisten) => unlisten()));
  });
}
