<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { commandKey } from '@/platform';
import type { CollectionTrack, PlaylistNode } from '@/types/collection';

/**
 * The playlists a track is in, each a pill with an × that takes the track out
 * of it, in a box that offers the other playlists as one is typed: picking one
 * adds the track to it.
 */
const props = defineProps<{ track: CollectionTrack }>();

const store = useCollectionStore();
const browse = useBrowseStore();

const box = ref<HTMLElement | null>(null);
const input = ref<HTMLInputElement | null>(null);
const list = ref<HTMLElement | null>(null);
const draft = ref('');
const open = ref(false);
/** The offer the arrow keys are on; -1 for none. */
const highlighted = ref(-1);
/** Where the list goes: under the box, or over it where there's no room. */
const place = ref<Record<string, string>>({});

const MAX_OFFERED = 50;
const SHOWN_AT_ONCE = 8;
const OFFER_HEIGHT = 24;

const byId = computed(() => new Map(store.playlists.map((p) => [p.id, p])));

/** The folders a playlist is in, outermost first: what tells two of a name apart. */
function folderOf(node: PlaylistNode): string {
  const folders: string[] = [];
  for (let at = byId.value.get(node.parentId ?? ''); at; at = byId.value.get(at.parentId ?? '')) {
    folders.unshift(at.name);
  }
  return folders.join(' / ');
}

/** The playlists the track is in, as the track's row says; the tree is newer where they differ. */
const members = computed(() =>
  props.track.playlistIds.map((id, at) => {
    const node = byId.value.get(id);
    return { id, name: node?.name ?? props.track.playlists[at] ?? '', folder: node ? folderOf(node) : '' };
  }),
);

/** Playlists the track isn't in: all of them until something is typed, then the ones that match, best first. */
const offered = computed(() => {
  if (!open.value) return [];
  const typed = draft.value.trim().toLowerCase();
  const inAlready = new Set(props.track.playlistIds);
  const starts: PlaylistNode[] = [];
  const words: PlaylistNode[] = [];
  const holds: PlaylistNode[] = [];
  for (const node of store.playlists) {
    if (node.kind !== 'playlist' || inAlready.has(node.id)) continue;
    const at = node.name.toLowerCase().indexOf(typed);
    if (at === -1) continue;
    if (at === 0) starts.push(node);
    else if (/[^\p{L}\p{N}]/u.test(node.name[at - 1])) words.push(node);
    else holds.push(node);
  }
  return [...starts, ...words, ...holds].slice(0, MAX_OFFERED).map((node) => ({ node, folder: folderOf(node) }));
});

function position() {
  const rect = box.value?.getBoundingClientRect();
  if (!rect) return;
  const height = Math.min(offered.value.length, SHOWN_AT_ONCE) * OFFER_HEIGHT + 10;
  const above = rect.bottom + height > window.innerHeight && rect.top - height > 0;
  place.value = {
    left: `${rect.left}px`,
    width: `${rect.width}px`,
    maxHeight: `${height}px`,
    ...(above ? { bottom: `${window.innerHeight - rect.top + 4}px` } : { top: `${rect.bottom + 4}px` }),
  };
}

watch(offered, (now) => {
  // Something typed is most likely the first match, so Return takes it.
  highlighted.value = now.length && draft.value.trim() ? 0 : -1;
  // Placed once the box has its new height: a pill added or removed moves its bottom edge.
  if (now.length) void nextTick(position);
});

watch(highlighted, (at) => {
  if (at >= 0) void nextTick(() => list.value?.children[at]?.scrollIntoView({ block: 'nearest' }));
});

function close() {
  open.value = false;
  highlighted.value = -1;
}

/** The page moving under the list would leave it behind. */
function onScroll(event: Event) {
  if (event.target instanceof Node && list.value?.contains(event.target)) return;
  close();
}
onMounted(() => {
  window.addEventListener('scroll', onScroll, true);
  window.addEventListener('resize', close);
});
onBeforeUnmount(() => {
  window.removeEventListener('scroll', onScroll, true);
  window.removeEventListener('resize', close);
});

// Another track starts with an empty box.
watch(
  () => props.track.id,
  () => {
    draft.value = '';
    close();
  },
);

/** Shown at once; if the change is refused, the list is read again and shows what is really there. */
function show(playlists: { id: string; name: string }[]) {
  browse.patchTrack(props.track.id, {
    playlists: playlists.map((p) => p.name),
    playlistIds: playlists.map((p) => p.id),
  });
}

function add(node: PlaylistNode) {
  const { id: trackId } = props.track;
  draft.value = '';
  show([...members.value, node]);
  void store.edit({ kind: 'addTracks', playlistId: node.id, trackIds: [trackId] });
}

function remove(playlistId: string) {
  const { id: trackId } = props.track;
  // A pill that had the keyboard hands it to the box, rather than drop it.
  if (box.value?.contains(document.activeElement) && document.activeElement !== input.value) input.value?.focus();
  show(members.value.filter((p) => p.id !== playlistId));
  void store.edit({ kind: 'removeTracks', playlistId, trackIds: [trackId] });
}

/** The last pill's ×: where ← and Backspace go from the empty box. */
function lastPill(): HTMLElement | null {
  const pills = box.value?.querySelectorAll<HTMLElement>('button');
  return pills?.length ? pills[pills.length - 1] : null;
}

function onBlur() {
  close();
  draft.value = '';
}

function onKey(event: KeyboardEvent) {
  // Keys that are still composing a character are the input method's; ones with ⌘ are shortcuts.
  if (event.isComposing || commandKey(event)) return;
  const offers = offered.value;
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    // With no playlists on offer, the arrows are the sidebar's: they go to its next entry.
    if (!offers.length) return;
    const step = event.key === 'ArrowDown' ? 1 : -1;
    highlighted.value = (Math.max(highlighted.value, step === 1 ? -1 : 0) + step + offers.length) % offers.length;
  } else if (event.key === 'Enter') {
    // Return offers the playlists, then takes the one the arrows are on.
    if (highlighted.value >= 0) add(offers[highlighted.value].node);
    else open.value = true;
  } else if (event.key === 'Escape') {
    // The first Escape puts the list away; the next leaves the box.
    if (open.value && offers.length) close();
    else input.value?.blur();
  } else if ((event.key === 'ArrowLeft' || event.key === 'Backspace') && !event.shiftKey && !event.altKey) {
    // From the empty box, onto the last pill: Backspace there takes the track out of it.
    const pill = draft.value ? null : lastPill();
    if (!pill) return;
    pill.focus();
  } else return;
  event.preventDefault();
  event.stopPropagation();
}
</script>

<template>
  <!-- Pressing the box anywhere but a pill's × puts the keyboard in it. -->
  <div
    ref="box"
    data-keys-group
    class="flex w-full cursor-text flex-wrap items-center gap-1 rounded border border-border bg-surface-1 p-1 focus-within:border-border-active"
    @mousedown="
      if ($event.target !== input) {
        $event.preventDefault();
        input?.focus();
        open = true;
      }
    "
  >
    <span
      v-for="playlist in members"
      :key="playlist.id"
      class="flex max-w-full items-center gap-0.5 rounded bg-surface-2 py-0.5 pr-0.5 pl-1.5 text-[11px] text-text-secondary"
      :title="playlist.folder ? `${playlist.folder} / ${playlist.name}` : playlist.name"
    >
      <span class="truncate">{{ playlist.name }}</span>
      <button
        class="shrink-0 rounded px-1 leading-none text-text-muted transition-colors hover:bg-surface-3 hover:text-text-primary"
        :title="`Take it out of ${playlist.name}. It stays in the collection.`"
        :aria-label="`Remove from ${playlist.name}`"
        @mousedown.stop.prevent
        @click="remove(playlist.id)"
        @keydown.delete.exact.stop.prevent="remove(playlist.id)"
      >×</button>
    </span>
    <input
      ref="input"
      v-model="draft"
      data-keys-current
      class="min-w-12 flex-1 bg-transparent px-1 py-0.5 text-xs outline-none placeholder:text-text-muted"
      :placeholder="members.length ? '' : 'Add to a playlist…'"
      aria-label="Add to a playlist"
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      @click="open = true"
      @blur="onBlur"
      @input="open = true"
      @keydown="onKey"
    />
  </div>
  <Teleport to="body">
    <ul
      v-if="offered.length"
      ref="list"
      class="fixed z-50 overflow-y-auto rounded border border-border bg-surface-1 py-1 text-xs text-text-primary shadow-lg"
      :style="place"
    >
      <!-- Pressed, not clicked: a click would take the keyboard from the box first. -->
      <li
        v-for="(offer, at) in offered"
        :key="offer.node.id"
        class="flex cursor-default items-baseline gap-2 px-2"
        :class="at === highlighted ? 'bg-accent/25' : 'hover:bg-surface-2'"
        :style="{ height: `${OFFER_HEIGHT}px`, lineHeight: `${OFFER_HEIGHT}px` }"
        @mousedown.prevent="add(offer.node)"
      >
        <span class="truncate">{{ offer.node.name }}</span>
        <span v-if="offer.folder" class="ml-auto max-w-[50%] shrink-0 truncate text-[10px] text-text-muted">{{ offer.folder }}</span>
      </li>
    </ul>
  </Teleport>
</template>
