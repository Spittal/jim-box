<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { confirm } from '@/composables/useConfirm';
import { showContextMenu, type MenuLine } from '@/composables/useContextMenu';
import { dragging, dropTree, endDrag, startDrag } from '@/composables/useDrag';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useJobsStore } from '@/stores/jobs';
import { isPlaylistLink, usePlaylistStore } from '@/stores/playlist';
import type { PlaylistNode } from '@/types/collection';
import type { SyncSettings } from '@/types/playlist';

/**
 * The playlist tree: picking a list, and making, naming, moving and filling
 * playlists. Each playlist has a button for linking it to a Spotify
 * playlist, which it then follows.
 */
const store = useCollectionStore();
const { status, playlists } = storeToRefs(store);
const browse = useBrowseStore();
const { selected, selectedNode } = storeToRefs(browse);
const jobs = useJobsStore();
const playlistStore = usePlaylistStore();
const { watched } = storeToRefs(playlistStore);

const collapsed = ref(new Set<string>());

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
      await store.edit({ kind: 'renamePlaylist', id: pending.id, name });
    }
  } else {
    const id = await store.edit({
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

// The keyboard, when the pane has it (see useDetailKeys.ts).

const pane = ref<HTMLElement | null>(null);

/** ↑ and ↓ pick the list above or below; ← and → close and open a folder. */
function onKey(event: KeyboardEvent) {
  if (event.defaultPrevented || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return;
  // A name being typed keeps its keys.
  if ((event.target as HTMLElement).closest('input')) return;
  if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
    const ids: (string | null)[] = [null, ...treeRows.value.map((node) => node.id)];
    const at = ids.indexOf(selected.value) + (event.key === 'ArrowDown' ? 1 : -1);
    if (at >= 0 && at < ids.length) {
      browse.select(ids[at]);
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
  const yes = await confirm({
    title: `Delete “${node.name}”?`,
    message:
      node.kind === 'folder'
        ? 'The folder and the playlists in it are deleted. Their tracks stay in the collection.'
        : 'The playlist is deleted. Its tracks stay in the collection.',
    confirm: 'Delete',
    danger: true,
  });
  if (yes) await store.edit({ kind: 'deletePlaylist', id: node.id });
}

/** A tree row's context menu; the Collection row's and the empty space's make new playlists at the top. */
function treeMenu(node: PlaylistNode | null) {
  if (node) browse.select(node.id);
  const lines: MenuLine[] = [
    { text: 'New Playlist', action: () => startCreate(false) },
    { text: 'New Folder', action: () => startCreate(true) },
  ];
  if (node) {
    lines.push('separator', { text: 'Rename', action: () => startRename(node) });
    if (node.kind === 'playlist') lines.push({ text: 'Playlist Settings…', action: () => (settingsOf.value = node.id) });
    lines.push('separator', { text: 'Delete…', action: () => void deletePlaylist(node) });
  }
  void showContextMenu(lines);
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
// folders or a new place among their siblings.

/** Whether `node` is `ancestorId` or inside it. */
function isWithin(node: PlaylistNode, ancestorId: string): boolean {
  for (let at: PlaylistNode | undefined = node; at; at = playlists.value.find((p) => p.id === at!.parentId)) {
    if (at.id === ancestorId) return true;
  }
  return false;
}

function acceptsDrop(node: PlaylistNode): boolean {
  const drag = dragging.value;
  if (!drag) return false;
  if (drag.kind === 'tracks') return node.kind === 'playlist' && node.id !== selected.value;
  if (drag.kind === 'result') return node.kind === 'playlist';
  // A folder can't go inside itself.
  return !isWithin(node, drag.id);
}

function overTree(event: DragEvent, node: PlaylistNode) {
  if (!acceptsDrop(node)) return;
  event.preventDefault();
  dropTree.value = node.id;
}

async function dropOnTree(node: PlaylistNode) {
  const drag = dragging.value;
  const accepted = acceptsDrop(node);
  endDrag();
  if (!drag || !accepted) return;
  if (drag.kind === 'tracks') {
    await store.edit({ kind: 'addTracks', playlistId: node.id, trackIds: drag.ids });
  } else if (drag.kind === 'result') {
    // One you have joins the playlist now; one you don't, when it lands.
    if (drag.ownedId) await store.edit({ kind: 'addTracks', playlistId: node.id, trackIds: [drag.ownedId] });
    else {
      try {
        await jobs.fetch(drag.track, node.id);
      } catch (e) {
        store.error = String(e);
      }
    }
  } else if (node.kind === 'folder') {
    await store.edit({ kind: 'movePlaylist', id: drag.id, parentId: node.id, index: null });
  } else {
    // Onto a playlist: take its place among its siblings.
    const siblings = playlists.value.filter((p) => p.parentId === node.parentId && p.id !== drag.id);
    await store.edit({
      kind: 'movePlaylist',
      id: drag.id,
      parentId: node.parentId,
      index: siblings.findIndex((p) => p.id === node.id),
    });
  }
}

const treeButtonClass = 'rounded px-1.5 py-0.5 text-[11px] text-text-muted hover:bg-surface-2 hover:text-text-primary';
const iconButtonClass =
  'shrink-0 rounded p-1 text-text-muted hover:bg-surface-2 hover:text-text-primary disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-text-muted';
const GEAR_ICON =
  'M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z';
const TRASH_ICON = 'M2.5 4.5h11M6 4.5V3h4v1.5M4 4.5l.5 9h7l.5-9M6.5 7v4M9.5 7v4';
const nameInputClass =
  'min-w-0 flex-1 rounded border border-border-active bg-surface-1 px-1 py-0 text-xs outline-none';
</script>

<template>
  <div
    ref="pane"
    data-pane="playlists"
    tabindex="-1"
    class="flex w-56 shrink-0 flex-col border-r border-border outline-none"
    @keydown="onKey"
  >
    <ul class="min-h-0 flex-1 overflow-y-auto py-1 text-xs select-none" @contextmenu.prevent="treeMenu(null)">
      <li
        class="flex cursor-default items-center gap-1 py-1 pr-2 pl-4"
        :class="selected === null ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1'"
        :aria-current="selected === null"
        @click="browse.select(null)"
      >
        <span class="min-w-0 flex-1 truncate">Collection</span>
        <span class="shrink-0 font-mono text-[10px] text-text-muted">{{ status?.tracks }}</span>
      </li>
      <li class="section-label px-4 pt-3 pb-1 text-[10px]">Playlists</li>
      <li v-if="naming && !naming.id && newRowAfter === null" class="flex items-center py-0.5 pr-2 pl-4">
        <input ref="nameInput" v-model="naming.name" :class="nameInputClass" spellcheck="false" @keydown.enter="commitName" @keydown.esc="naming = null" @blur="commitName" />
      </li>
      <template v-for="node in treeRows" :key="node.id">
        <li
          class="group flex cursor-default items-center gap-1 py-1 pr-2"
          :class="[
            selected === node.id ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1',
            dropTree === node.id ? 'outline outline-1 -outline-offset-1 outline-accent' : '',
          ]"
          :style="{ paddingLeft: `${node.depth * 12 + 4}px` }"
          :aria-current="selected === node.id"
          :draggable="naming?.id !== node.id"
          @click="browse.select(node.id)"
          @dblclick="startRename(node)"
          @contextmenu.prevent.stop="treeMenu(node)"
          @dragstart="startDrag($event, { kind: 'playlist', id: node.id })"
          @dragend="endDrag"
          @dragover="overTree($event, node)"
          @dragleave="dropTree === node.id && (dropTree = null)"
          @drop.prevent="dropOnTree(node)"
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
            <span class="min-w-0 flex-1 truncate" :class="{ 'font-medium': node.kind === 'folder' }" :title="node.name">
              {{ node.name }}
            </span>
            <button
              v-if="node.kind === 'playlist'"
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
    <Teleport to="body">
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
                Linked to “{{ followed.name }}”, read every minute while jim-box is open.
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
    <!-- Icons that are always there, so picking a playlist or asking to delete one moves nothing. -->
    <div class="flex shrink-0 items-center gap-1 border-t border-border p-1.5">
      <button :class="treeButtonClass" class="whitespace-nowrap" title="New playlist" @click="startCreate(false)">+ Playlist</button>
      <button :class="treeButtonClass" class="whitespace-nowrap" title="New folder" @click="startCreate(true)">+ Folder</button>
      <span class="flex-1" />
        <button
          :class="iconButtonClass"
          :disabled="!selectedNode"
          :title="selectedNode ? `Rename “${selectedNode.name}”` : 'Rename (pick a playlist first)'"
          aria-label="Rename"
          @click="selectedNode && startRename(selectedNode)"
        >
          <svg viewBox="0 0 16 16" class="size-3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M11 2.5l2.5 2.5L5.5 13H3v-2.5l8-8zM9.5 4l2.5 2.5" />
          </svg>
        </button>
        <button
          :class="iconButtonClass"
          :disabled="!selectedNode"
          :title="!selectedNode
            ? 'Delete (pick a playlist first)'
            : selectedNode.kind === 'folder'
              ? 'Delete this folder and the playlists in it. Tracks stay in the collection.'
              : 'Delete this playlist. Its tracks stay in the collection.'"
          aria-label="Delete"
          @click="selectedNode && deletePlaylist(selectedNode)"
        >
          <svg viewBox="0 0 16 16" class="size-3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path :d="TRASH_ICON" />
          </svg>
        </button>
    </div>
  </div>
</template>
