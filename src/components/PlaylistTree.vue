<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { dragging, dropTree, endDrag, startDrag } from '@/composables/useDrag';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useJobsStore } from '@/stores/jobs';
import type { PlaylistNode } from '@/types/collection';

/** The playlist tree: picking a list, and making, naming, moving and filling playlists. */
const store = useCollectionStore();
const { status, playlists } = storeToRefs(store);
const browse = useBrowseStore();
const { selected, selectedNode } = storeToRefs(browse);
const jobs = useJobsStore();

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
const confirmingDelete = ref<string | null>(null);
watch(selected, () => (confirmingDelete.value = null));

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

async function deletePlaylist(id: string) {
  confirmingDelete.value = null;
  await store.edit({ kind: 'deletePlaylist', id });
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
const nameInputClass =
  'min-w-0 flex-1 rounded border border-border-active bg-surface-1 px-1 py-0 text-xs outline-none';
</script>

<template>
  <div class="flex w-56 shrink-0 flex-col border-r border-border">
    <ul class="min-h-0 flex-1 overflow-y-auto py-1 text-xs">
      <li
        class="flex cursor-default items-center gap-1 py-1 pr-2 pl-4"
        :class="selected === null ? 'bg-surface-2 text-text-primary' : 'text-text-secondary hover:bg-surface-1'"
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
          :draggable="naming?.id !== node.id"
          @click="browse.select(node.id)"
          @dblclick="startRename(node)"
          @dragstart="startDrag($event, { kind: 'playlist', id: node.id })"
          @dragend="endDrag"
          @dragover="overTree($event, node)"
          @dragleave="dropTree === node.id && (dropTree = null)"
          @drop.prevent="dropOnTree(node)"
        >
          <button
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
    <div class="flex shrink-0 flex-wrap items-center gap-1 border-t border-border p-1.5">
      <button :class="treeButtonClass" title="New playlist" @click="startCreate(false)">+ Playlist</button>
      <button :class="treeButtonClass" title="New folder" @click="startCreate(true)">+ Folder</button>
      <template v-if="selectedNode">
        <template v-if="confirmingDelete === selectedNode.id">
          <span class="px-1 text-[11px] text-text-secondary">Delete “{{ selectedNode.name }}”?</span>
          <button :class="treeButtonClass" class="text-status-error" @click="deletePlaylist(selectedNode.id)">Delete</button>
          <button :class="treeButtonClass" @click="confirmingDelete = null">Cancel</button>
        </template>
        <template v-else>
          <button :class="treeButtonClass" @click="startRename(selectedNode)">Rename</button>
          <button
            :class="treeButtonClass"
            :title="selectedNode.kind === 'folder'
              ? 'Delete this folder and the playlists in it. Tracks stay in the collection.'
              : 'Delete this playlist. Its tracks stay in the collection.'"
            @click="confirmingDelete = selectedNode.id"
          >
            Delete
          </button>
        </template>
      </template>
    </div>
  </div>
</template>
