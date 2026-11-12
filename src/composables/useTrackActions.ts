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

  /** Takes the selected tracks out of the playlist being shown. They stay in the collection. */
  async function removeFromPlaylist() {
    const trackIds = browse.selectedIds;
    const playlistId = browse.selected;
    if (!browse.inPlaylist || !playlistId || !trackIds.length) return;
    browse.clearSelection();
    await collection.edit({ kind: 'removeTracks', playlistId, trackIds });
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
    browse.clearSelection();
    await collection.edit({ kind: 'deleteTracks', trackIds, trashFiles: trash || answer.option });
  }

  return { identify, analyse, review, reviewable, allReviewed, addToPlaylist, removeFromPlaylist, remove };
}
