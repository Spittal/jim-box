<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { confirm } from '@/composables/useConfirm';
import { showContextMenu, type MenuLine } from '@/composables/useContextMenu';
import { dragging, dropTree, endDrag, startDrag, type Drag } from '@/composables/useDrag';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useJobsStore } from '@/stores/jobs';
import { libraryOf, useLibrariesStore } from '@/stores/libraries';
import { isPlaylistLink, usePlaylistStore } from '@/stores/playlist';
import { useUiStore } from '@/stores/ui';
import { commandKey, revealLabel } from '@/platform';
import type { PlaylistNode } from '@/types/collection';
import type { SyncSettings } from '@/types/playlist';

/**
 * One library's playlist tree: picking a list, and making, naming, moving
 * and filling playlists. The master's playlists each have a button for
 * linking them to a Spotify playlist, which they then follow. Another
 * library's (`library`) is the same, with a title over it saying which it
 * is; tracks and playlists dragged between them go from one to the other.
 */
const props = defineProps<{
  /** The library shown: null for the master. */
  library: string | null;
  /** Whether to say which library it is, as when there's more than one. */
  titled: boolean;
}>();

const store = useCollectionStore();
const libraries = useLibrariesStore();
const browse = useBrowseStore();
const jobs = useJobsStore();
const ui = useUiStore();
const playlistStore = usePlaylistStore();
const { watched } = storeToRefs(playlistStore);

const other = computed(() => libraries.get(props.library));
const kind = computed(() => libraries.kindOf(props.library));
const isStick = computed(() => kind.value === 'stick');
const title = computed(() => (props.library ? (other.value?.name ?? '') : 'Library'));
const playlists = computed(() => (other.value ? other.value.playlists : store.playlists));
const status = computed(() => (other.value ? other.value.status : store.status));
/** Whether the list shown is this library's. */
const active = computed(() => browse.library === props.library);
const lists = computed(() => (active.value ? browse.lists : []));
const selected = computed(() => (active.value ? browse.selected : null));
const selectedNode = computed(() => (active.value ? browse.selectedNode : null));

/** Makes a change to this library. */
const edit = (change: Parameters<typeof store.edit>[0]) => store.edit(change, { library: props.library });

const collapsed = ref(new Set<string>());

/** Shift- and ⌘-clicks pick lists; they don't select text. */
function keepTextUnselected(event: MouseEvent) {
  if (!event.shiftKey && !commandKey(event)) return;
  if ((event.target as HTMLElement).closest('input')) return;
  event.preventDefault();
  window.getSelection()?.removeAllRanges();
}

/** Tree rows, without the descendants of collapsed folders. */
const treeRows = computed(() => {
  const rows: PlaylistNode[] = [];
  let hiddenBelow: number | null = null;
  for (const node of playlists.value) {
    if (hiddenBelow !== null) {
      if (node.depth > hiddenBelow) continue;
      hiddenBelow = null;
    }
    rows.push(node);
    if (node.kind === 'folder' && collapsed.value.has(node.id)) hiddenBelow = node.depth;
  }
  return rows;
});

// Picking lists: a click picks one; with ⌘ (Ctrl on Windows) or Shift, several, shown as one list.

/** Where a Shift-click's range starts: the list last clicked without Shift. */
let anchor: string | null = null;
/** The list last clicked, which ↑ and ↓ move from. */
const clicked = ref<string | null>(null);
const lead = computed(() =>
  clicked.value && lists.value.includes(clicked.value) ? clicked.value : (lists.value[lists.value.length - 1] ?? null),
);

/** Shows this library's whole collection. */
function showCollection() {
  anchor = null;
  browse.select(null, props.library);
}

function clickNode(node: PlaylistNode, event: MouseEvent) {
  clicked.value = node.id;
  if (event.shiftKey && active.value) {
    const ids = treeRows.value.map((row) => row.id);
    const from = ids.indexOf(anchor ?? node.id);
    const to = ids.indexOf(node.id);
    // The anchor has gone, or is folded away: just this one.
    if (from === -1) browse.select((anchor = node.id));
    else browse.selectSeveral(ids.slice(Math.min(from, to), Math.max(from, to) + 1));
  } else if (commandKey(event) && active.value) {
    anchor = node.id;
    browse.selectSeveral(
      lists.value.includes(node.id) ? lists.value.filter((id) => id !== node.id) : [...lists.value, node.id],
    );
  } else browse.select((anchor = node.id));
}

function toggleFolder(id: string) {
  const next = new Set(collapsed.value);
  if (!next.delete(id)) next.add(id);
  collapsed.value = next;
}

// Creating, renaming and deleting playlists.

/** The tree row whose name is being typed: a new one, or one being renamed. */
const naming = ref<{ id: string | null; folder: boolean; parentId: string | null; name: string } | null>(null);
const nameInput = ref<HTMLInputElement[] | HTMLInputElement | null>(null);

async function focusNameInput() {
  await nextTick();
  const input = Array.isArray(nameInput.value) ? nameInput.value[0] : nameInput.value;
  input?.focus();
  input?.select();
}

/** New playlists go in the selected folder, or beside the selected playlist. */
function startCreate(folder: boolean) {
  const node = selectedNode.value;
  const parentId = node ? (node.kind === 'folder' ? node.id : node.parentId) : null;
  if (parentId && collapsed.value.has(parentId)) toggleFolder(parentId);
  naming.value = { id: null, folder, parentId, name: folder ? 'New folder' : 'New playlist' };
  focusNameInput();
}

function startRename(node: PlaylistNode) {
  naming.value = { id: node.id, folder: node.kind === 'folder', parentId: node.parentId, name: node.name };
  focusNameInput();
}

async function commitName() {
  const pending = naming.value;
  if (!pending) return;
  naming.value = null;
  const name = pending.name.trim();
  if (!name) return;
  if (pending.id) {
    if (playlists.value.find((p) => p.id === pending.id)?.name !== name) {
      await edit({ kind: 'renamePlaylist', id: pending.id, name });
    }
  } else {
    const id = await edit({
      kind: 'createPlaylist',
      name,
      parentId: pending.parentId,
      folder: pending.folder,
    });
    if (id) browse.select(id);
  }
}

// A playlist's settings, in a dialog: linking it to a Spotify playlist.

/** The playlist whose settings are open. */
const settingsOf = ref<string | null>(null);
const settingsNode = computed(() => playlists.value.find((p) => p.id === settingsOf.value) ?? null);
/** Its link to Spotify, if it has one. */
const followed = computed(() => (settingsOf.value ? watched.value.get(settingsOf.value) ?? null : null));
const linkUrl = ref('');
/** What the last one-time import into it did. */
const imported = ref<string | null>(null);
watch(settingsOf, () => {
  linkUrl.value = '';
  imported.value = null;
});
// Deleted while its settings were open.
watch(settingsNode, (node) => !node && (settingsOf.value = null));

async function act(action: () => Promise<unknown>) {
  store.error = null;
  try {
    await action();
  } catch (e) {
    store.error = String(e);
  }
}

function linkSpotify() {
  const id = settingsOf.value;
  const url = linkUrl.value.trim();
  if (!id || !isPlaylistLink(url)) return;
  act(async () => {
    await playlistStore.link(url, id);
    linkUrl.value = '';
  });
}

/** Brings the Spotify playlist's tracks in once, without following it. */
function importSpotify() {
  const id = settingsOf.value;
  const url = linkUrl.value.trim();
  if (!id || !isPlaylistLink(url)) return;
  imported.value = null;
  act(async () => {
    const done = await playlistStore.importOnce(url, id);
    const count = (n: number) => `${n} ${n === 1 ? 'track' : 'tracks'}`;
    const parts = [`${count(done.added)} added`];
    if (done.present) parts.push(`${done.present} already here`);
    if (done.fetching) parts.push(`${done.fetching} being fetched`);
    if (done.skipped) parts.push(`${done.skipped} left out (no sure match, or fetched before)`);
    if (done.truncated) parts.push('only the first 100 tracks could be read');
    // Still the same playlist's settings.
    if (settingsOf.value === id) {
      imported.value = `Imported “${done.name}”: ${parts.join(' · ')}.`;
      linkUrl.value = '';
    }
  });
}

function setSetting(change: Partial<SyncSettings>) {
  const id = settingsOf.value;
  const current = followed.value?.settings;
  if (id && current) act(() => playlistStore.setSettings(id, { ...current, ...change }));
}

/** Turning 1-to-1 sync on asks first; turning it off doesn't. */
async function toggleMirror(event: Event) {
  const box = event.target as HTMLInputElement;
  if (!box.checked) return setSetting({ mirror: false });
  box.checked = false;
  const yes = await confirm({
    title: 'Turn on 1-to-1 sync?',
    message:
      'This removes tracks from this playlist. Every track that isn’t on the Spotify playlist is taken out now, ' +
      'including ones you added yourself, and tracks later removed on Spotify are taken out too. They stay in your collection.',
    confirm: 'Turn on 1-to-1 sync',
    danger: true,
  });
  if (yes) setSetting({ mirror: true });
}

function unlinkSpotify() {
  const id = settingsOf.value;
  if (id) act(() => playlistStore.unlink(id));
}

// The keyboard, when the pane has it (see useDetailKeys.ts): the sidebar hands keys to the tree being browsed.

const pane = ref<HTMLElement | null>(null);

/** ↑ and ↓ pick the list above or below; ← and → close and open a folder. */
function onKey(event: KeyboardEvent) {
  if (event.defaultPrevented || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return;
  // A name being typed keeps its keys.
  if ((event.target as HTMLElement).closest('input')) return;
  if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
    const ids: (string | null)[] = [null, ...treeRows.value.map((node) => node.id)];
    const at = ids.indexOf(lead.value) + (event.key === 'ArrowDown' ? 1 : -1);
    if (at >= 0 && at < ids.length) {
      const id = (anchor = ids[at]);
      browse.select(id, props.library);
      void nextTick(() => pane.value?.querySelector('[aria-current="true"]')?.scrollIntoView({ block: 'nearest' }));
    }
  } else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    // On anything but a folder, they're the player's.
    const node = selectedNode.value;
    if (node?.kind !== 'folder') return;
    if (collapsed.value.has(node.id) === (event.key === 'ArrowRight')) toggleFolder(node.id);
  } else return;
  event.preventDefault();
  event.stopPropagation();
}

const closeOnEscape = (event: KeyboardEvent) => event.key === 'Escape' && (settingsOf.value = null);
onMounted(() => window.addEventListener('keydown', closeOnEscape));
onUnmounted(() => window.removeEventListener('keydown', closeOnEscape));

/** Deletes a playlist or folder, once asked. */
async function deletePlaylist(node: PlaylistNode) {
  const where = isStick.value ? 'stick' : 'collection';
  const yes = await confirm({
    title: `Delete “${node.name}”?`,
    message:
      (node.kind === 'folder'
        ? `The folder and the playlists in it are deleted. Their tracks stay on the ${where}.`
        : `The playlist is deleted. Its tracks stay on the ${where}.`) +
      (isStick.value ? ' Nothing changes on the stick until you apply the changes.' : ''),
    confirm: 'Delete',
    danger: true,
  });
  if (yes) await edit({ kind: 'deletePlaylist', id: node.id });
}

// --- Between libraries --------------------------------------------------------

/** The other open libraries a playlist or track of this one can go to. */
const elsewhere = computed(() => {
  const targets: { id: string | null; name: string; kind: string }[] = [];
  if (props.library) targets.push({ id: null, name: 'Library', kind: 'master' });
  for (const o of libraries.others) {
    if (o.id === props.library) continue;
    // Only the master's go onto a stick: what's on one is the master's.
    if (o.kind === 'stick' && props.library) continue;
    targets.push({ id: o.id, name: o.name, kind: o.kind });
  }
  return targets;
});

function sendTo(target: { id: string | null; name: string }, ids: string[]): MenuLine {
  return { text: target.id ? target.name : 'Library', action: () => void libraries.movePlaylists(ids, target.id) };
}

/** A tree row's context menu; the Collection row's and the empty space's make new playlists at the top. */
function treeMenu(node: PlaylistNode | null) {
  // A right-click on one of several being shown leaves them showing.
  if (node && !lists.value.includes(node.id)) browse.select((anchor = node.id));
  const lines: MenuLine[] = [
    { text: 'New Playlist', action: () => startCreate(false) },
    { text: 'New Folder', action: () => startCreate(true) },
  ];
  if (node) {
    lines.push('separator', { text: 'Rename', action: () => startRename(node) });
    if (node.kind === 'playlist' && !props.library) lines.push({ text: 'Playlist Settings…', action: () => (settingsOf.value = node.id) });
    const picked = lists.value.includes(node.id) ? lists.value : [node.id];
    if (elsewhere.value.length && !node.pending) {
      lines.push('separator');
      if (props.library) lines.push({ text: 'Import into Library', action: () => void libraries.movePlaylists(picked, null) });
      const targets = elsewhere.value.filter((t) => t.id !== null);
      if (targets.length) lines.push({ text: 'Send to', items: targets.map((t) => sendTo(t, picked)) });
    }
    lines.push('separator', { text: 'Delete…', action: () => void deletePlaylist(node) });
  }
  void showContextMenu(lines);
}

/** The title's menu: what can be done with the library as a whole. */
function libraryMenu() {
  const lib = other.value;
  if (!lib) {
    void showContextMenu([{ text: 'Open Another Library…', action: () => void libraries.open() }]);
    return;
  }
  const lines: MenuLine[] = [
    {
      text: lib.kind === 'stick' ? 'Import or Update Library from This Stick' : 'Import or Update Library from This One',
      action: () => void importAll(),
    },
  ];
  if (lib.kind === 'stick') {
    lines.push({ text: 'Update Stick from Library', action: () => void libraries.updateStick(lib.id) });
    if (lib.staged?.changes) {
      lines.push(
        'separator',
        { text: 'Apply Changes…', action: () => (ui.usb = true) },
        { text: 'Discard Changes…', action: () => void discard() },
      );
    }
  }
  lines.push(
    'separator',
    { text: revealLabel, action: () => void revealItemInDir(lib.path).catch((e) => (libraries.error = String(e))) },
    { text: 'Read Again', action: () => void libraries.reload(lib.id) },
    { text: 'Close', action: () => void libraries.close(lib.id) },
  );
  void showContextMenu(lines);
}

async function importAll() {
  const lib = other.value;
  if (!lib) return;
  const yes = await confirm({
    title: `Bring everything on ${lib.name} into your library?`,
    message:
      `Every track and playlist on it comes into your library. Tracks you don't have are copied in. ` +
      `Ones you have, and playlists with the same names in the same folders, are updated to match ${lib.name}: ` +
      'its beat grids, cues, ratings and colours replace yours.',
    confirm: 'Import',
  });
  if (yes) await libraries.importAll(lib.id);
}

async function discard() {
  const lib = other.value;
  if (!lib) return;
  const yes = await confirm({
    title: `Discard the changes waiting for ${lib.name}?`,
    message: 'They were never written to the stick, so it stays as it is.',
    confirm: 'Discard',
    danger: true,
  });
  if (yes) await libraries.discard(lib.id);
}

/** Where a new row's name box goes: after its parent's last descendant. */
const newRowAfter = computed(() => {
  const pending = naming.value;
  if (!pending || pending.id) return undefined;
  if (!pending.parentId) return treeRows.value.length ? treeRows.value[treeRows.value.length - 1].id : null;
  const rows = treeRows.value;
  const parentAt = rows.findIndex((r) => r.id === pending.parentId);
  if (parentAt === -1) return null;
  let last = parentAt;
  while (last + 1 < rows.length && rows[last + 1].depth > rows[parentAt].depth) last++;
  return rows[last].id;
});
const newRowDepth = computed(() => {
  const parent = playlists.value.find((p) => p.id === naming.value?.parentId);
  return parent ? parent.depth + 1 : 0;
});

// Drops: tracks and search results onto a playlist, and playlists into
// folders or a new place among their siblings. From another library, they
// come into this one.

/** Whether `node` is `ancestorId` or inside it. */
function isWithin(node: PlaylistNode, ancestorId: string): boolean {
  for (let at: PlaylistNode | undefined = node; at; at = playlists.value.find((p) => p.id === at!.parentId)) {
    if (at.id === ancestorId) return true;
  }
  return false;
}

/** The library a drag is from; undefined for what isn't of one (files, search results). */
function fromLibrary(drag: Drag): string | null | undefined {
  if (drag.kind === 'tracks') return libraryOf(drag.ids[0]);
  if (drag.kind === 'playlists') return libraryOf(drag.ids[0]);
  return undefined;
}

/** Whether a drag comes from another library, and could come into this one. */
function crossing(drag: Drag): boolean | null {
  const from = fromLibrary(drag);
  if (from === undefined || from === props.library) return false;
  // Only the master's go onto a stick.
  if (isStick.value && from !== null) return null;
  return true;
}

function acceptsDrop(node: PlaylistNode | null): boolean {
  const drag = dragging.value;
  if (!drag) return false;
  const across = crossing(drag);
  if (across === null) return false;
  if (across) {
    if (drag.kind === 'tracks') return node ? node.kind === 'playlist' : !isStick.value;
    // A playlist goes under a folder, beside a playlist, or onto the library's top.
    return true;
  }
  if (!node) return false;
  if (drag.kind === 'tracks') return node.kind === 'playlist' && node.id !== selected.value;
  if (drag.kind === 'result' || drag.kind === 'files') return node.kind === 'playlist' && !props.library;
  // Playlists on a stick stay where they are; a folder can't go inside itself.
  return !isStick.value && !drag.ids.some((id) => isWithin(node, id));
}

/** What dragging a row takes: every list picked, if it's one of them, but not those inside another picked. */
function dragRow(event: DragEvent, node: PlaylistNode) {
  let ids = [node.id];
  if (lists.value.length > 1 && lists.value.includes(node.id)) {
    const picked = new Set(lists.value);
    ids = playlists.value
      .filter((p) => picked.has(p.id) && !p.pending)
      .filter((p) => ![...picked].some((other) => other !== p.id && isWithin(p, other)))
      .map((p) => p.id);
  }
  startDrag(event, { kind: 'playlists', ids });
}

function overTree(event: DragEvent, node: PlaylistNode | null) {
  if (!acceptsDrop(node)) return;
  event.preventDefault();
  dropTree.value = node ? node.id : `top:${props.library ?? ''}`;
}

async function dropOnTree(node: PlaylistNode | null) {
  const drag = dragging.value;
  const accepted = acceptsDrop(node);
  endDrag();
  // Files from outside are taken by `useFileDrop`, before this hears of the drop.
  if (!drag || !accepted || drag.kind === 'files') return;
  if (crossing(drag)) {
    if (drag.kind === 'tracks') await libraries.moveTracks(drag.ids, props.library, node?.id ?? null);
    else if (drag.kind === 'playlists') {
      const under = node ? (node.kind === 'folder' ? node.id : node.parentId) : null;
      await libraries.movePlaylists(drag.ids, props.library, under);
    }
    return;
  }
  if (!node) return;
  if (drag.kind === 'tracks') {
    await edit({ kind: 'addTracks', playlistId: node.id, trackIds: drag.ids });
  } else if (drag.kind === 'result') {
    // One you have joins the playlist now; one you don't, when it lands.
    if (drag.ownedId) await edit({ kind: 'addTracks', playlistId: node.id, trackIds: [drag.ownedId] });
    else {
      try {
        await jobs.fetch(drag.track, node.id);
      } catch (e) {
        store.error = String(e);
      }
    }
  } else if (drag.kind === 'playlists') {
    // Several move as one change, undone together, keeping their order.
    const group = drag.ids.length > 1 ? `move-playlists-${Date.now()}` : null;
    for (const id of drag.ids) {
      if (id === node.id) continue;
      if (node.kind === 'folder') {
        await store.edit({ kind: 'movePlaylist', id, parentId: node.id, index: null }, { group, library: props.library });
      } else {
        // Onto a playlist: in just before it among its siblings, after any moved already.
        const siblings = playlists.value.filter((p) => p.parentId === node.parentId && p.id !== id);
        await store.edit(
          { kind: 'movePlaylist', id, parentId: node.parentId, index: siblings.findIndex((p) => p.id === node.id) },
          { group, library: props.library },
        );
      }
    }
  }
}

/** A word on what's waiting to be written to a stick. */
const stagedNote = computed(() => {
  const staged = other.value?.staged;
  if (!staged?.changes) return null;
  return `${staged.changes} ${staged.changes === 1 ? 'change' : 'changes'} waiting to be written. Apply them from USB at the bottom.`;
});

defineExpose({ onKey });

const iconButtonClass =
  'shrink-0 rounded p-1 text-text-muted hover:bg-surface-2 hover:text-text-primary disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-text-muted';
const GEAR_ICON =
  'M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z';
const NEW_PLAYLIST_ICON = 'M2 4h8M2 8h8M2 12h5M12.5 9.5v5M10 12h5';
const NEW_FOLDER_ICON = 'M1.5 3.5h4.5l1.5 1.5h7v8h-13zM8 7.5v4M6 9.5h4';
const STICK_ICON = 'M5 1.5h6v4H5zM3.5 5.5h9v8a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1zM6.5 3v1M9.5 3v1';
const LIBRARY_ICON = 'M2.5 2.5h3v11h-3zM6.5 2.5h3v11h-3zM10.5 3l2.6-.6 2.4 10.6-2.6.6z';
const nameInputClass =
  'min-w-0 flex-1 rounded border border-border-active bg-surface-1 px-1 py-0 text-xs outline-none';
</script>

<template>
  <div ref="pane" class="flex min-h-0 flex-col">
    <!-- Which library this is, when more than one is open. Right-click it for what can be done with it. -->
    <div
      v-if="titled"
      class="group flex shrink-0 cursor-default items-center gap-1.5 border-y border-border bg-surface-1 py-1 pr-1 pl-3 text-[11px] select-none"
      :class="dropTree === `top:${library ?? ''}` ? 'outline outline-1 -outline-offset-1 outline-accent' : ''"
      :title="other ? other.path : (status?.masterDb ?? '')"
      @contextmenu.prevent="libraryMenu"
      @dragover="overTree($event, null)"
      @dragleave="dropTree === `top:${library ?? ''}` && (dropTree = null)"
      @drop.prevent="dropOnTree(null)"
    >
      <svg viewBox="0 0 16 16" class="size-3.5 shrink-0 text-text-muted" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" aria-hidden="true">
        <path :d="isStick ? STICK_ICON : LIBRARY_ICON" />
      </svg>
      <span class="min-w-0 flex-1 truncate font-medium text-text-primary">{{ library ? title : 'Master library' }}</span>
      <span
        v-if="other?.staged?.changes"
        class="shrink-0 rounded-full bg-accent px-1.5 font-mono text-[10px] text-white"
        :title="stagedNote ?? ''"
      >{{ other.staged.changes }}</span>
      <button :class="iconButtonClass" title="What can be done with this library" aria-label="Library menu" @click.stop="libraryMenu">
        <svg viewBox="0 0 16 16" class="size-3.5" fill="currentColor" aria-hidden="true">
          <circle cx="3.5" cy="8" r="1.2" /><circle cx="8" cy="8" r="1.2" /><circle cx="12.5" cy="8" r="1.2" />
        </svg>
      </button>
      <button v-if="library" :class="iconButtonClass" title="Close this library" aria-label="Close" @click.stop="libraries.close(library)">
        <svg viewBox="0 0 16 16" class="size-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
      </button>
    </div>
    <ul class="min-h-0 flex-1 overflow-y-auto py-1 text-xs select-none" @contextmenu.prevent="treeMenu(null)">
      <li
        class="flex cursor-default items-center gap-1 py-1 pr-2 pl-4"
        :class="[
          active && !lists.length ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1',
          !titled && dropTree === `top:${library ?? ''}` ? 'outline outline-1 -outline-offset-1 outline-accent' : '',
        ]"
        :aria-current="active && !lists.length"
        @click="showCollection"
        @dragover="overTree($event, null)"
        @dragleave="dropTree === `top:${library ?? ''}` && (dropTree = null)"
        @drop.prevent="dropOnTree(null)"
      >
        <span class="min-w-0 flex-1 truncate">{{ isStick ? 'All tracks' : 'Collection' }}</span>
        <span class="shrink-0 font-mono text-[10px] text-text-muted">{{ status?.tracks }}</span>
      </li>
      <li class="flex items-center gap-0.5 pt-2 pr-1.5 pb-0.5 pl-4">
        <span class="section-label flex-1 text-[10px]">Playlists</span>
        <button :class="iconButtonClass" title="New playlist" aria-label="New playlist" @click.stop="startCreate(false)">
          <svg viewBox="0 0 16 16" class="size-3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path :d="NEW_PLAYLIST_ICON" />
          </svg>
        </button>
        <button :class="iconButtonClass" title="New folder" aria-label="New folder" @click.stop="startCreate(true)">
          <svg viewBox="0 0 16 16" class="size-3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path :d="NEW_FOLDER_ICON" />
          </svg>
        </button>
      </li>
      <li v-if="naming && !naming.id && newRowAfter === null" class="flex items-center py-0.5 pr-2 pl-4">
        <input ref="nameInput" v-model="naming.name" :class="nameInputClass" spellcheck="false" @keydown.enter="commitName" @keydown.esc="naming = null" @blur="commitName" />
      </li>
      <template v-for="node in treeRows" :key="node.id">
        <li
          class="group flex cursor-default items-center gap-1 py-1 pr-2"
          :class="[
            lists.includes(node.id) ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1',
            dropTree === node.id ? 'outline outline-1 -outline-offset-1 outline-accent' : '',
          ]"
          :style="{ paddingLeft: `${node.depth * 12 + 4}px` }"
          :aria-current="lead === node.id"
          :aria-selected="lists.includes(node.id)"
          :draggable="naming?.id !== node.id && !node.pending"
          @mousedown="keepTextUnselected"
          @click="clickNode(node, $event)"
          @dblclick="startRename(node)"
          @contextmenu.prevent.stop="treeMenu(node)"
          @dragstart="dragRow($event, node)"
          @dragend="endDrag"
          @dragover.stop="overTree($event, node)"
          @dragleave="dropTree === node.id && (dropTree = null)"
          @drop.prevent.stop="dropOnTree(node)"
        >
          <button
            tabindex="-1"
            class="w-3 shrink-0 text-[9px] text-text-muted"
            :class="{ invisible: node.kind !== 'folder' }"
            @click.stop="toggleFolder(node.id)"
          >
            {{ collapsed.has(node.id) ? '▶' : '▼' }}
          </button>
          <input
            v-if="naming?.id === node.id"
            ref="nameInput"
            v-model="naming.name"
            :class="nameInputClass"
            spellcheck="false"
            @click.stop
            @dblclick.stop
            @keydown.enter="commitName"
            @keydown.esc="naming = null"
            @blur="commitName"
          />
          <template v-else>
            <span
              class="min-w-0 flex-1 truncate"
              :class="{ 'font-medium': node.kind === 'folder', italic: node.pending }"
              :title="node.pending ? `${node.name} (waiting to be written to the stick)` : node.name"
            >
              {{ node.name }}
            </span>
            <span v-if="node.pending" class="size-1.5 shrink-0 rounded-full bg-accent" aria-label="Waiting to be written" />
            <button
              v-if="node.kind === 'playlist' && !library"
              tabindex="-1"
              class="shrink-0 rounded p-0.5 hover:bg-surface-3"
              :class="watched.has(node.id) ? 'text-accent' : 'text-text-muted opacity-0 group-hover:opacity-100 focus:opacity-100'"
              :title="watched.has(node.id) ? `Playlist settings. Follows “${watched.get(node.id)!.name}” on Spotify.` : 'Playlist settings'"
              aria-label="Playlist settings"
              @click.stop="settingsOf = node.id"
              @dblclick.stop
            >
              <svg viewBox="0 0 24 24" class="size-3" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path :d="GEAR_ICON" />
                <circle cx="12" cy="12" r="3" />
              </svg>
            </button>
            <span v-if="node.kind === 'smart'" class="shrink-0 text-[10px] text-text-muted" title="Intelligent playlist">auto</span>
            <span v-else-if="node.count !== null" class="shrink-0 font-mono text-[10px] text-text-muted">{{ node.count }}</span>
          </template>
        </li>
        <li
          v-if="naming && !naming.id && newRowAfter === node.id"
          class="flex items-center py-0.5 pr-2"
          :style="{ paddingLeft: `${newRowDepth * 12 + 16}px` }"
        >
          <input ref="nameInput" v-model="naming.name" :class="nameInputClass" spellcheck="false" @keydown.enter="commitName" @keydown.esc="naming = null" @blur="commitName" />
        </li>
      </template>
      <li v-if="!playlists.length && !naming" class="px-4 py-2 text-text-muted">No playlists yet.</li>
    </ul>
    <!-- A playlist's settings, over the whole window. -->
    <Teleport v-if="!library" to="body">
      <div
        v-if="settingsNode"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
        @click.self="settingsOf = null"
      >
        <div
          role="dialog"
          aria-modal="true"
          :aria-label="`Settings for ${settingsNode.name}`"
          class="flex max-h-full w-full max-w-md flex-col gap-4 overflow-y-auto rounded-lg border border-border bg-surface-1 p-5 text-sm shadow-xl"
        >
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0">
              <h2 class="truncate font-medium">{{ settingsNode.name }}</h2>
              <p class="text-xs text-text-muted">Playlist settings</p>
            </div>
            <button :class="iconButtonClass" title="Close" aria-label="Close" @click="settingsOf = null">
              <svg viewBox="0 0 16 16" class="size-3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
                <path d="M4 4l8 8M12 4l-8 8" />
              </svg>
            </button>
          </div>

          <section class="flex flex-col gap-3">
            <h3 class="section-label text-[10px]">Spotify</h3>

            <!-- Not linked yet. -->
            <template v-if="!followed">
              <p class="text-xs text-text-secondary">
                Link this playlist to a public Spotify playlist. Tracks on it that you have are added here, the rest
                are fetched, and tracks added on Spotify later follow. Or import it once: the same, without
                following it afterwards.
              </p>
              <form class="flex gap-2" @submit.prevent="linkSpotify">
                <input
                  v-model="linkUrl"
                  type="text"
                  spellcheck="false"
                  placeholder="Paste a Spotify playlist link"
                  class="min-w-0 flex-1 rounded border border-border bg-surface-0 px-3 py-1.5 text-sm outline-none placeholder:text-text-muted focus:border-accent"
                />
                <button
                  type="submit"
                  class="shrink-0 rounded bg-accent px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
                  title="Follow the Spotify playlist: tracks added to it later are brought in too."
                  :disabled="!isPlaylistLink(linkUrl) || playlistStore.importing"
                >
                  Link
                </button>
                <button
                  type="button"
                  class="shrink-0 rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
                  title="Bring its tracks in now, without following it. Needs rekordbox to be closed."
                  :disabled="!isPlaylistLink(linkUrl) || playlistStore.importing"
                  @click="importSpotify"
                >
                  Import once
                </button>
              </form>
              <p v-if="playlistStore.importing" class="text-xs text-text-muted">
                <template v-if="playlistStore.progress?.total">
                  Matching tracks… {{ playlistStore.progress.done }} of {{ playlistStore.progress.total }}
                </template>
                <template v-else>Reading…</template>
              </p>
              <p v-else-if="imported" class="text-xs text-text-secondary">{{ imported }}</p>
            </template>

            <!-- Linked: how it follows. -->
            <template v-else>
              <p class="truncate text-xs text-text-secondary" :title="followed.name">
                Linked to “{{ followed.name }}”, read every minute while Jim Box is open.
              </p>
              <label class="flex cursor-pointer items-start gap-2">
                <input
                  type="checkbox"
                  class="mt-0.5 size-4 accent-accent"
                  :checked="followed.settings.autoFetch"
                  @change="setSetting({ autoFetch: ($event.target as HTMLInputElement).checked })"
                />
                <span>
                  Fetch tracks you don't have
                  <span class="block text-xs text-text-muted">Only when the match is sure. Others are listed for you to check.</span>
                </span>
              </label>
              <label class="flex cursor-pointer items-start gap-2">
                <input type="checkbox" class="mt-0.5 size-4 accent-accent" :checked="followed.settings.mirror" @change="toggleMirror" />
                <span>
                  1-to-1 sync
                  <span class="block text-xs" :class="followed.settings.mirror ? 'text-status-error' : 'text-text-muted'">
                    {{ followed.settings.mirror
                      ? 'On: tracks that aren’t on the Spotify playlist are removed from this playlist.'
                      : 'Keep only what is on the Spotify playlist. Off, nothing is ever removed.' }}
                  </span>
                </span>
              </label>
              <button
                class="self-start rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3"
                title="Stop following the Spotify playlist. This playlist and its tracks stay as they are."
                @click="unlinkSpotify"
              >
                Unlink from Spotify
              </button>
            </template>

            <ul class="list-disc space-y-0.5 border-t border-border pt-3 pl-4 text-xs text-text-muted">
              <li>A Spotify playlist can be linked to one playlist at a time.</li>
              <li>Track order isn't synced, only which tracks are in the playlist.</li>
              <li>Only a playlist's first 100 tracks can be read.</li>
              <li>Changes to a linked playlist wait while rekordbox is open. Importing once needs it closed.</li>
            </ul>
          </section>
        </div>
      </div>
    </Teleport>
  </div>
</template>
