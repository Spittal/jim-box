import { computed, onMounted, onUnmounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { dragging, dropTree, endDrag } from '@/composables/useDrag';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isWindows } from '@/platform';

/** Adds files, and the music in folders, to the collection (and a playlist), and says how it went. */
export async function addFromPaths(paths: string[], playlistId: string | null) {
  if (!paths.length) return;
  const browse = useBrowseStore();
  browse.notice = null;
  const report = await useCollectionStore().importPaths(paths, playlistId);
  if (!report) return;
  const parts = [`Added ${report.added} ${report.added === 1 ? 'track' : 'tracks'}`];
  if (report.existing) parts.push(`${report.existing} already in the collection`);
  if (report.skipped.length) parts.push(`${report.skipped.length} couldn't be read`);
  browse.notice = `${parts.join(' · ')}.`;
}

/** WebView2's way to hand the app what a page can't read itself: here, where dropped files are. */
type WebviewBridge = { postMessageWithAdditionalObjects(message: unknown, objects: ArrayLike<unknown>): void };

/**
 * Lets files and folders dragged in from Finder or Explorer be dropped on the
 * window: they join the collection, and the playlist they were dropped on in
 * the sidebar or else the one that's open. While `ready()` is false a drop
 * does nothing (rather than have the web view open the file).
 *
 * Returns where a drop would put the files being dragged over the window:
 * a playlist's name, '' for the collection alone, null when none are.
 */
export function useFileDrop(ready: () => boolean) {
  const browse = useBrowseStore();
  const collection = useCollectionStore();

  /** A drag that began in the window: never files from outside, though dragged pictures say they are. */
  let own = false;
  /** How many elements the drag is inside: none once it has left the window. */
  let depth = 0;
  /** The playlist of the drop whose paths are still on their way (Windows). */
  let pendingPlaylist: string | null = null;

  const carriesFiles = (event: DragEvent) => !own && !!event.dataTransfer?.types.includes('Files');
  const isOurs = () => dragging.value?.kind === 'files';

  function onStart() {
    own = true;
  }
  function onEnd() {
    own = false;
  }

  function onEnter(event: DragEvent) {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    depth++;
    if (ready() && !dragging.value) dragging.value = { kind: 'files' };
  }

  function onOver(event: DragEvent) {
    if (!carriesFiles(event)) return;
    // Without this the web view takes the drop itself, and opens the file.
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = isOurs() ? 'copy' : 'none';
  }

  function onLeave(event: DragEvent) {
    if (!carriesFiles(event)) return;
    depth = Math.max(0, depth - 1);
    if (!depth && isOurs()) endDrag();
  }

  // Before any element hears of it: none of them knows what to do with files.
  function onDrop(event: DragEvent) {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    event.stopPropagation();
    depth = 0;
    if (!isOurs()) return;
    const playlistId = dropTree.value ?? (browse.inPlaylist ? browse.selected : null);
    endDrag();
    if (isWindows) {
      // The paths come back as a `files-dropped` event.
      const bridge = (window as unknown as { chrome?: { webview?: WebviewBridge } }).chrome?.webview;
      pendingPlaylist = playlistId;
      bridge?.postMessageWithAdditionalObjects({ dropped: true }, event.dataTransfer?.files ?? []);
    } else {
      void invoke<string[]>('dropped_paths').then(
        (paths) => addFromPaths(paths, playlistId),
        (e) => (collection.error = String(e)),
      );
    }
  }

  const unlisten = listen<string[]>('files-dropped', (e) => addFromPaths(e.payload, pendingPlaylist));

  onMounted(() => {
    window.addEventListener('dragstart', onStart, true);
    window.addEventListener('dragend', onEnd, true);
    window.addEventListener('dragenter', onEnter);
    window.addEventListener('dragover', onOver);
    window.addEventListener('dragleave', onLeave);
    window.addEventListener('drop', onDrop, true);
  });
  onUnmounted(() => {
    window.removeEventListener('dragstart', onStart, true);
    window.removeEventListener('dragend', onEnd, true);
    window.removeEventListener('dragenter', onEnter);
    window.removeEventListener('dragover', onOver);
    window.removeEventListener('dragleave', onLeave);
    window.removeEventListener('drop', onDrop, true);
    unlisten.then((stop) => stop());
  });

  return computed(() => {
    if (!isOurs()) return null;
    const id = dropTree.value ?? (browse.inPlaylist ? browse.selected : null);
    return (id && collection.playlists.find((p) => p.id === id)?.name) || '';
  });
}
