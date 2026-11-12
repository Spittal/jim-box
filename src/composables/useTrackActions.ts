import { confirm } from '@/composables/useConfirm';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useLibraryStore } from '@/stores/library';
import { trashName } from '@/platform';

/** Whether deleting tracks last sent their files to the Trash too; it does until told otherwise. */
const TRASH_KEY = 'tracks.trashOnDelete';

/**
 * What can be done to the selected tracks, from the toolbar, the keyboard or
 * a row's context menu.
 */
export function useTrackActions() {
  const collection = useCollectionStore();
  const browse = useBrowseStore();
  const library = useLibraryStore();

  /** Looks the selected tracks up on MusicBrainz; what it finds shows beside the list. */
  async function identify() {
    collection.error = null;
    try {
      await library.identify(browse.selectedIds);
    } catch (e) {
      collection.error = String(e);
    }
  }

  /** Analyses what in the selection has no analysis; asks before redoing it when all of it has. */
  async function analyse() {
    const trackIds = browse.selectedIds;
    if (!trackIds.length) return;
    if ((await collection.analyse(trackIds, false)) !== 0) return;
    const one = trackIds.length === 1;
    const yes = await confirm({
      title: one ? 'Analyse this track again?' : `Analyse these ${trackIds.length} tracks again?`,
      message: `${one ? 'It already has' : 'They already have'} an analysis. Cues are kept. ${
        one ? 'Its beat grid is' : 'Their beat grids are'
      } replaced, including any adjusted by hand in rekordbox.`,
      confirm: 'Analyse again',
    });
    if (yes) await collection.analyse(trackIds, true);
  }

  /** The selected tracks that have a grid to review: all but those seen to have no analysis. */
  function reviewable(): string[] {
    return browse.selectedIds.filter((id) => browse.loadedTrack(id)?.analysed !== false);
  }

  /** Whether every selected track with a grid has had it reviewed, so that reviewing them would take it back. */
  function allReviewed(): boolean {
    const trackIds = reviewable();
    return trackIds.length > 0 && trackIds.every((id) => browse.loadedTrack(id)?.grid?.reviewed);
  }

  /** Marks the selected tracks' beat grids as reviewed; or, when they all are, takes that back. */
  async function review() {
    const trackIds = reviewable();
    if (!trackIds.length) return;
    collection.error = null;
    try {
      await collection.reviewGrids(trackIds, !allReviewed());
    } catch (e) {
      collection.error = String(e);
    }
  }

  async function addToPlaylist(playlistId: string) {
    const trackIds = browse.selectedIds;
    if (trackIds.length) await collection.edit({ kind: 'addTracks', playlistId, trackIds });
  }

  /**
   * What to play once these tracks have left the list, when the one playing
   * is among them: the first under it that's staying. Asked before they go.
   */
  async function successor(trackIds: string[]) {
    const playing = browse.opened;
    if (!playing || !browse.playing || !trackIds.includes(playing.id)) return null;
    const next = await browse.trackPast(playing.id, new Set(trackIds)).catch(() => null);
    // Unless another track was opened, or the player closed, meanwhile.
    return () => {
      if (!next || browse.opened !== playing) return;
      browse.opened = next;
      browse.selectTracks([next.id]);
    };
  }

  /** Takes the selected tracks out of the playlists being shown. They stay in the collection. */
  async function removeFromPlaylist() {
    const trackIds = browse.selectedIds;
    const playlists = browse.removableFrom;
    if (!playlists.length || !trackIds.length) return;
    const playNext = await successor(trackIds);
    browse.clearSelection();
    let done = true;
    // One thing to undo, however many playlists.
    const group = crypto.randomUUID();
    for (const [at, playlistId] of playlists.entries()) {
      // The lists are read again once, after the last.
      const quiet = at < playlists.length - 1;
      done &&= (await collection.edit({ kind: 'removeTracks', playlistId, trackIds }, { quiet, group })) !== undefined;
    }
    if (done) playNext?.();
  }

  /**
   * Takes the selected tracks out of the collection, once asked. Their files
   * go to the Trash if wanted; with `trash`, they go without that being asked.
   */
  async function remove(trash = false) {
    const trackIds = browse.selectedIds;
    if (!trackIds.length) return;
    const one = trackIds.length === 1;
    const title = one ? browse.loadedTrack(trackIds[0])?.title : null;
    const what = one ? `${title ? `“${title}”` : 'this track'}` : `${trackIds.length} tracks`;
    const leaves = `${one ? 'It leaves' : 'They leave'} the collection and every playlist, with ${
      one ? 'its' : 'their'
    } cues and beat ${one ? 'grid' : 'grids'}`;
    const answer = await confirm({
      title: trash ? `Delete ${what} and ${one ? 'its file' : 'their files'}?` : `Delete ${what} from the collection?`,
      message: trash ? `${leaves}, and ${one ? 'its file goes' : 'their files go'} to the ${trashName}.` : `${leaves}.`,
      option: trash
        ? undefined
        : {
            label: `Also move ${one ? 'its file' : 'their files'} to the ${trashName}`,
            checked: localStorage.getItem(TRASH_KEY) !== 'false',
          },
      confirm: trash ? 'Delete with file' : 'Delete',
      danger: true,
    });
    if (!answer) return;
    // What was ticked is remembered; a delete that didn't ask says nothing of it.
    if (!trash) localStorage.setItem(TRASH_KEY, String(answer.option));
    const playNext = await successor(trackIds);
    browse.clearSelection();
    const done = await collection.edit({ kind: 'deleteTracks', trackIds, trashFiles: trash || answer.option });
    if (done !== undefined) playNext?.();
  }

  return { identify, analyse, review, reviewable, allReviewed, addToPlaylist, removeFromPlaylist, remove };
}
