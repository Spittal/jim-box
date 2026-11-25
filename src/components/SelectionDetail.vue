<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue';
import FieldInput from '@/components/FieldInput.vue';
import { onDetailKey } from '@/composables/useDetailKeys';
import { formatDuration } from '@/composables/useFormat';
import { formatKey as keyIn, useKeyNotation } from '@/composables/useKeyNotation';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { libraryOf, useLibrariesStore } from '@/stores/libraries';
import { useUiStore } from '@/stores/ui';
import { commandKey } from '@/platform';
import type { CollectionEdit, CollectionTrack } from '@/types/collection';

/**
 * The sidebar for several selected tracks: what they have in common, `…`
 * where they differ. Whatever is set here is set on all of them, as one
 * thing to undo.
 */
const props = defineProps<{
  /** The selected tracks; null while some are still being read. */
  tracks: CollectionTrack[] | null;
  /** How many are selected. */
  count: number;
}>();
defineEmits<{ close: [] }>();

const store = useCollectionStore();
const browse = useBrowseStore();
const libraries = useLibrariesStore();
const ui = useUiStore();
const { formatKey, allKeys } = useKeyNotation();

const all = computed(() => props.tracks ?? []);
/** A stick's tracks (or ones on their way to a stick) are as the stick has them. */
const readOnly = computed(
  () =>
    !props.tracks ||
    all.value.some((t) => t.pending) ||
    (all.value.length > 0 && libraries.kindOf(libraryOf(all.value[0].id)) === 'stick'),
);

/** What every track has; undefined where they differ, or before they're read. */
function shared<T>(of: (track: CollectionTrack) => T): T | undefined {
  const [first, ...rest] = all.value;
  if (!first) return undefined;
  const value = of(first);
  return rest.every((track) => of(track) === value) ? value : undefined;
}

type TextField = 'title' | 'artist' | 'album' | 'genre';
const fields: { id: TextField; label: string }[] = [
  { id: 'title', label: 'Title' },
  { id: 'artist', label: 'Artist' },
  { id: 'album', label: 'Album' },
  { id: 'genre', label: 'Genre' },
];

const text = computed(() => Object.fromEntries(fields.map((f) => [f.id, shared((t) => t[f.id])])));
const keyName = computed(() => shared((t) => keyIn(t.key, 'standard')));
const rating = computed(() => shared((t) => t.rating));
const color = computed(() => shared((t) => t.color));
const comment = computed(() => shared((t) => t.comment));

/**
 * Changes every track that doesn't already have it: shown at once, then
 * written together.
 */
function setAll(
  differs: (track: CollectionTrack) => boolean,
  patch: Partial<CollectionTrack>,
  edit: (trackId: string) => CollectionEdit,
) {
  if (readOnly.value) return;
  const changing = all.value.filter(differs);
  if (!changing.length) return;
  browse.patchTracks(new Map(changing.map((t) => [t.id, patch])));
  store.edit({ kind: 'several', edits: changing.map((t) => edit(t.id)) });
}

/** A field left as it was, `…` and all, changes nothing; anything typed goes on every track. */
function commitField(field: TextField, value: string) {
  if (text.value[field] === undefined && value === '') return;
  setAll(
    (t) => t[field] !== value,
    { [field]: value },
    (trackId) => ({ kind: 'setField', trackId, field, value }),
  );
}

/** Stands for the keys differing, in the menu. */
const MIXED = '(mixed)';

/** The keys to choose from. One they all have that isn't a key is kept on the menu, so it shows. */
const keyChoices = computed(() => {
  const keys = allKeys();
  const name = keyName.value;
  return !name || keys.includes(name) ? keys : [name, ...keys];
});

function chooseKey(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (value === MIXED) return;
  setAll(
    (t) => keyIn(t.key, 'standard') !== value,
    { key: value },
    (trackId) => ({ kind: 'setField', trackId, field: 'key', value }),
  );
}

/** Clicking the rating they all have clears it, as for one track. */
function rate(stars: number) {
  const next = rating.value === stars ? 0 : stars;
  setAll(
    (t) => t.rating !== next,
    { rating: next },
    (trackId) => ({ kind: 'setRating', trackId, stars: next }),
  );
}

function colour(id: number) {
  setAll(
    (t) => t.color !== id,
    { color: id },
    (trackId) => ({ kind: 'setColor', trackId, color: id }),
  );
}

function commitComment(event: Event) {
  const value = (event.target as HTMLTextAreaElement).value;
  if (comment.value === undefined && value === '') return;
  setAll(
    (t) => t.comment !== value,
    { comment: value },
    (trackId) => ({ kind: 'setComment', trackId, comment: value }),
  );
}

/** ⌘1 to ⌘5 give them all their stars, as they do one track. */
function onWindowKey(event: KeyboardEvent) {
  if (event.defaultPrevented || event.altKey || ui.settings || ui.usb || ui.downloads) return;
  if (!commandKey(event) || event.shiftKey || !/^Digit[1-5]$/.test(event.code)) return;
  if (!event.repeat) rate(Number(event.code.slice(5)));
  event.preventDefault();
}
onMounted(() => window.addEventListener('keydown', onWindowKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onWindowKey));

/** Keyed by the selection, so the boxes start afresh when another is made. */
const selectionKey = computed(() => all.value.map((t) => t.id).join());

const inputClass =
  'w-full rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none focus:border-border-active placeholder:text-text-muted';
</script>

<template>
  <!-- Takes the keyboard as the one track's sidebar does (see `focusPane`). -->
  <aside
    data-track-detail
    data-pane="detail"
    tabindex="-1"
    class="flex w-64 shrink-0 flex-col gap-3 overflow-y-auto scroll-py-8 border-l border-border p-3 text-xs outline-none"
    @keydown="onDetailKey"
  >
    <div class="flex items-center justify-between">
      <span class="section-label">{{ count }} selected</span>
      <button class="text-text-muted hover:text-text-primary" title="Close" tabindex="-1" @click="$emit('close')">✕</button>
    </div>

    <p v-if="!tracks" class="text-text-muted">Reading the tracks…</p>
    <template v-else>
      <p v-if="readOnly" class="rounded bg-surface-1 px-2 py-1.5 text-[11px] text-text-secondary">
        As the USB stick has them. Change them in your library, then update the stick.
      </p>
      <p v-else class="text-[11px] text-text-muted">
        <span class="font-mono">…</span> where they differ. What you set here is set on all of them.
      </p>

      <fieldset :key="selectionKey" :disabled="readOnly" class="contents">
        <label v-for="field in fields" :key="field.id" class="flex flex-col gap-1">
          <span class="text-[11px] text-text-muted">{{ field.label }}</span>
          <FieldInput
            :field="field.id"
            :value="text[field.id] ?? ''"
            :placeholder="text[field.id] === undefined ? '…' : undefined"
            :class="inputClass"
            :data-keys-first="field.id === 'artist' ? '' : undefined"
            arrows-leave
            @commit="commitField(field.id, $event)"
          />
        </label>

        <label class="flex flex-col gap-1">
          <span class="text-[11px] text-text-muted">Key</span>
          <select :value="keyName ?? MIXED" :class="[inputClass, 'font-mono']" @change="chooseKey">
            <option v-if="keyName === undefined" :value="MIXED" disabled>…</option>
            <option value="">–</option>
            <option v-for="key in keyChoices" :key="key" :value="key">{{ formatKey(key) }}</option>
          </select>
        </label>

        <div class="flex flex-col gap-1">
          <span class="text-[11px] text-text-muted">Rating</span>
          <div data-keys-group class="flex items-center gap-0.5 text-base leading-none">
            <button
              v-for="star in 5"
              :key="star"
              :data-keys-current="star === rating ? '' : undefined"
              :class="rating !== undefined && star <= rating ? 'text-text-primary' : 'text-text-muted/40 hover:text-text-muted'"
              :title="`${star} ${star === 1 ? 'star' : 'stars'} on all of them`"
              @click="rate(star)"
            >★</button>
            <span v-if="rating === undefined" class="ml-1 font-mono text-xs text-text-muted">…</span>
          </div>
        </div>

        <label class="flex flex-col gap-1">
          <span class="text-[11px] text-text-muted">Comment</span>
          <textarea
            :value="comment ?? ''"
            :placeholder="comment === undefined ? '…' : undefined"
            :class="inputClass"
            rows="3"
            @change="commitComment"
          />
        </label>

        <div class="flex flex-col gap-1">
          <span class="text-[11px] text-text-muted">Colour</span>
          <div data-keys-group class="flex items-center gap-1.5">
            <button
              v-for="(c, id) in TRACK_COLORS"
              :key="id"
              :data-keys-current="color === id ? '' : undefined"
              class="size-4 rounded-full border"
              :class="color === id ? 'border-text-primary' : 'border-border'"
              :style="{ backgroundColor: c.hex || 'transparent' }"
              :title="`${c.name} on all of them`"
              @click="colour(id)"
            />
            <span v-if="color === undefined" class="ml-0.5 font-mono text-xs text-text-muted">…</span>
          </div>
        </div>
      </fieldset>

      <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-t border-border pt-3 text-[11px]">
        <dt class="text-text-muted">Time</dt>
        <dd class="font-mono">{{ browse.selectionSecs ? formatDuration(browse.selectionSecs) : '–' }}</dd>
      </dl>
    </template>
  </aside>
</template>
