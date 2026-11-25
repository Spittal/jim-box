import { ref } from 'vue';
import type { TrackRequest } from '@/types/track';

/** What is being dragged: collection tracks, playlists, an online search result, or files from outside the app. */
export type Drag =
  | { kind: 'tracks'; ids: string[] }
  /** Several when the row dragged is one of several picked; in tree order, none inside another. */
  | { kind: 'playlists'; ids: string[] }
  /** `ownedId` is the collection track the result already is, if any. */
  | { kind: 'result'; track: TrackRequest; ownedId: string | null }
  /** See `useFileDrop`, which also takes the drop. */
  | { kind: 'files' };

// One drag at a time, shared by the playlist tree, the track list and the results.
export const dragging = ref<Drag | null>(null);
/** The playlist tree row a drop would land on. */
export const dropTree = ref<string | null>(null);
/** The track row a drop would land before. */
export const dropRow = ref<number | null>(null);

export function startDrag(event: DragEvent, drag: Drag) {
  dragging.value = drag;
  // WebKit only starts a drag that carries data.
  event.dataTransfer?.setData('text/plain', drag.kind);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'copyMove';
}

export function endDrag() {
  dragging.value = null;
  dropTree.value = null;
  dropRow.value = null;
}
