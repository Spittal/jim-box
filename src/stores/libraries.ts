import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open as pickFolder } from '@tauri-apps/plugin-dialog';
import { showToast } from '@/composables/useToast';
import { useUsbStore } from './usb';
import type { Applied, LibraryStatus, Moved, OtherKind, OtherLibrary, PluggedLibrary, TransferProgress } from '@/types/libraries';
import type { UsbConvert } from '@/types/usb';

/** What joins another library's id to an id of its own: `lib1~123`. The master's ids have none. */
const SEP = '~';

/** The library an id is of: null for the master. */
export function libraryOf(id: string | null | undefined): string | null {
  if (!id) return null;
  const at = id.indexOf(SEP);
  return at === -1 ? null : id.slice(0, at);
}

const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** What moving tracks or playlists between libraries did, in a few words. */
export function describeMove(moved: Moved, where: string): string {
  if (moved.staged) return `Waiting to be written to ${where}. Apply the changes from USB at the bottom.`;
  const parts: string[] = [];
  if (moved.added) parts.push(`${count(moved.added, 'track')} copied in`);
  if (moved.updated) parts.push(`${count(moved.updated, 'track')} brought up to date`);
  if (moved.failed.length) parts.push(`${moved.failed.length} couldn't be`);
  return parts.length ? `${where}: ${parts.join(' · ')}` : `Nothing to bring into ${where}`;
}

/**
 * The rekordbox libraries: the one Jim Box manages (the master, in
 * rekordbox's default folder on this computer), and the others opened
 * beside it, USB sticks and other master.db files.
 */
export const useLibrariesStore = defineStore('libraries', () => {
  const status = ref<LibraryStatus | null>(null);
  /** The libraries open beside the master, in the order opened. */
  const others = ref<OtherLibrary[]>([]);
  /** Drives plugged in with a library on them that isn't open. */
  const plugged = ref<PluggedLibrary[]>([]);
  /** Bumped when another library changes, so a list of it is read again. */
  const generation = ref(0);
  /** Something being brought from one library to another, said in a few words. */
  const moving = ref<string | null>(null);
  /** How the move is going, once it has said. */
  const transfer = ref<TransferProgress | null>(null);
  /** When copying began, and how much was copied then, for how long the rest will take. */
  let copyStart: { at: number; bytes: number } | null = null;
  /** Seconds the copying has left, once it has gone long enough to tell. */
  const secondsLeft = ref<number | null>(null);

  function progressed(p: TransferProgress) {
    // One that arrives after the move is over says nothing new.
    if (!moving.value) return;
    transfer.value = p;
    if (p.stage !== 'copying') {
      copyStart = null;
      secondsLeft.value = null;
      return;
    }
    const now = Date.now();
    copyStart ??= { at: now, bytes: p.bytesDone };
    const seconds = (now - copyStart.at) / 1000;
    const copied = p.bytesDone - copyStart.bytes;
    secondsLeft.value = seconds >= 5 && copied > 0 ? ((p.bytesTotal - p.bytesDone) * seconds) / copied : null;
  }

  /** How far through its stage the move is, 0 to 100; null when the stage doesn't count. */
  const transferPercent = computed(() => {
    const p = transfer.value;
    if (!p) return null;
    if (p.stage === 'copying' && p.bytesTotal) return Math.round((p.bytesDone / p.bytesTotal) * 100);
    return p.total ? Math.round((p.done / p.total) * 100) : null;
  });
  const error = ref<string | null>(null);

  async function load() {
    status.value = await invoke<LibraryStatus>('library_status');
  }

  function get(id: string | null): OtherLibrary | null {
    return id ? (others.value.find((o) => o.id === id) ?? null) : null;
  }

  /** What a library is: the master, another master.db, or a stick. */
  function kindOf(id: string | null): 'master' | OtherKind {
    return get(id)?.kind ?? 'master';
  }

  /** What to call a library: "Library" for the master. */
  function nameOf(id: string | null): string {
    return get(id)?.name ?? 'Library';
  }

  /** The sticks with changes waiting to be written. */
  const staged = computed(() => others.value.filter((o) => o.kind === 'stick' && o.staged?.changes));
  const stagedCount = computed(() => staged.value.reduce((sum, o) => sum + (o.staged?.changes ?? 0), 0));

  /** Reads which libraries are open, and what's in them. */
  async function refresh() {
    try {
      others.value = await invoke<OtherLibrary[]>('other_libraries');
      generation.value++;
    } catch (e) {
      error.value = String(e);
    }
  }

  async function refreshPlugged() {
    try {
      plugged.value = await invoke<PluggedLibrary[]>('plugged_libraries');
    } catch {
      plugged.value = [];
    }
  }

  function replace(info: OtherLibrary) {
    const at = others.value.findIndex((o) => o.id === info.id);
    if (at === -1) others.value = [...others.value, info];
    else others.value = others.value.map((o) => (o.id === info.id ? info : o));
    generation.value++;
  }

  /** Puts a library's playlists and counts in place after a change to it. */
  function updated(id: string, change: Pick<OtherLibrary, 'status' | 'playlists'>) {
    const was = get(id);
    if (was) replace({ ...was, ...change });
  }

  /** Opens the libraries at `path`, or in a folder picked in the system's dialog. */
  async function open(path?: string): Promise<OtherLibrary[] | undefined> {
    error.value = null;
    const chosen = path ?? (await pickFolder({ directory: true, title: 'Open a rekordbox library or USB stick' }));
    if (typeof chosen !== 'string') return undefined;
    try {
      const opened = await invoke<OtherLibrary[]>('open_other_library', { path: chosen });
      opened.forEach(replace);
      void refreshPlugged();
      return opened;
    } catch (e) {
      error.value = String(e);
      return undefined;
    }
  }

  async function close(id: string) {
    error.value = null;
    try {
      await invoke('close_other_library', { id });
      others.value = others.value.filter((o) => o.id !== id);
      generation.value++;
      void refreshPlugged();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function reload(id: string) {
    error.value = null;
    try {
      replace(await invoke<OtherLibrary>('reload_other_library', { id }));
    } catch (e) {
      error.value = String(e);
    }
  }

  /** Runs a move between libraries, saying how it went. */
  async function move(what: string, to: string | null, run: () => Promise<Moved>): Promise<Moved | undefined> {
    error.value = null;
    moving.value = what;
    transfer.value = null;
    copyStart = null;
    secondsLeft.value = null;
    try {
      const moved = await run();
      showToast(describeMove(moved, nameOf(to)));
      if (moved.failed.length) error.value = `Some couldn't be brought in: ${moved.failed.slice(0, 5).join('; ')}${moved.failed.length > 5 ? '…' : ''}`;
      await refresh();
      return moved;
    } catch (e) {
      error.value = String(e);
      return undefined;
    } finally {
      moving.value = null;
      transfer.value = null;
    }
  }

  /** Tracks into another library: into one of its playlists, or just its collection. */
  function moveTracks(trackIds: string[], to: string | null, playlistId: string | null) {
    const what = `Bringing ${count(trackIds.length, 'track')} into ${nameOf(to)}…`;
    return move(what, to, () => invoke<Moved>('move_tracks', { trackIds, to, playlistId }));
  }

  /** Playlists or folders into another library, under one of its folders or at its top. */
  function movePlaylists(playlistIds: string[], to: string | null, under: string | null = null) {
    const what = `Bringing ${count(playlistIds.length, 'playlist')} into ${nameOf(to)}…`;
    return move(what, to, () => invoke<Moved>('move_playlists', { playlistIds, to, under }));
  }

  /** Every playlist and track of another library into the master. */
  function importAll(id: string) {
    return move(`Bringing ${nameOf(id)} into your library…`, null, () => invoke<Moved>('import_other_library', { id }));
  }

  /** Lines up a stick's playlists from the master to be written again. */
  async function updateStick(id: string) {
    error.value = null;
    try {
      replace(await invoke<OtherLibrary>('update_stick', { id }));
      showToast(`Updates waiting for ${nameOf(id)}. Apply the changes from USB at the bottom.`);
    } catch (e) {
      error.value = String(e);
    }
  }

  async function discard(id: string) {
    error.value = null;
    try {
      replace(await invoke<OtherLibrary>('discard_stick_changes', { id }));
    } catch (e) {
      error.value = String(e);
    }
  }

  /** Writes a stick's changes waiting, showing how it goes as an export does. */
  async function apply(id: string, eject: boolean, convert: UsbConvert = null): Promise<Applied | undefined> {
    const usb = useUsbStore();
    const stick = get(id);
    if (!stick || usb.exporting) return undefined;
    error.value = null;
    usb.exporting = stick.path;
    usb.progress = null;
    usb.report = null;
    usb.error = null;
    try {
      const applied = await invoke<Applied>('apply_stick_changes', { id, eject, convert });
      if (applied.problems.length) error.value = `${stick.name}: ${applied.problems.slice(0, 5).join('; ')}`;
      if (applied.ejectError) error.value = `${stick.name} was written, but couldn't be ejected: ${applied.ejectError}`;
      showToast(applied.ejected ? `${stick.name} is written and ejected` : `${stick.name} is written`);
      return applied;
    } catch (e) {
      error.value = String(e);
      return undefined;
    } finally {
      usb.exporting = null;
      usb.progress = null;
      await refresh();
      void usb.load();
    }
  }

  return {
    status,
    others,
    plugged,
    generation,
    moving,
    transfer,
    transferPercent,
    secondsLeft,
    progressed,
    error,
    staged,
    stagedCount,
    load,
    get,
    kindOf,
    nameOf,
    refresh,
    refreshPlugged,
    updated,
    open,
    close,
    reload,
    moveTracks,
    movePlaylists,
    importAll,
    updateStick,
    discard,
    apply,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useLibrariesStore, import.meta.hot));
