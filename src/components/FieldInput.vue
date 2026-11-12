<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useCollectionStore } from '@/stores/collection';
import { commandKey } from '@/platform';
import type { TrackField } from '@/types/collection';

/**
 * A box for one of a track's text fields. An artist, album or genre offers the
 * ones the collection already has as it's typed. Leaving the box or pressing
 * Return commits what's in it; Escape puts back what was there.
 */
defineOptions({ inheritAttrs: false });
const props = defineProps<{
  field: TrackField;
  value: string;
  /** Take the keyboard as soon as it appears, with the text selected. */
  autofocus?: boolean;
  /** ↑ and ↓ are left to whoever shows the box unless suggestions are showing, rather than opening them. */
  arrowsLeave?: boolean;
  /** Commit what's typed if the box is taken away while it has the keyboard, as when its row scrolls off. */
  commitOnRemove?: boolean;
}>();
const emit = defineEmits<{
  /** The box was left with this in it, trimmed; it may be what was there already. */
  commit: [value: string];
  /** Escape was pressed: nothing is to change. */
  cancel: [];
}>();

const store = useCollectionStore();
const input = ref<HTMLInputElement | null>(null);
const draft = ref(props.value);
const focused = ref(false);
/** Set once the box has given its answer, so leaving it doesn't give another. */
let settled = false;

watch(
  () => props.value,
  (value) => {
    if (!focused.value) draft.value = value;
  },
);

// --- Names to offer -----------------------------------------------------------

const MAX_OFFERED = 8;
const OFFER_HEIGHT = 24;

const suggests = computed(() => props.field === 'artist' || props.field === 'album' || props.field === 'genre');
/** The names in the collection, the most used first, with each as it's compared. */
const names = ref<{ name: string; folded: string }[]>([]);
const open = ref(false);
/** The offer the arrow keys are on; -1 for none. */
const highlighted = ref(-1);
/** Where the list goes: under the box, or over it where there's no room. */
const place = ref<Record<string, string>>({});

async function loadNames() {
  if (!suggests.value) return;
  const found = await store.namesOf(props.field as 'artist' | 'album' | 'genre');
  names.value = found.map((name) => ({ name, folded: name.toLowerCase() }));
}

/** Names starting with what's typed, then ones with a word starting with it, then ones just holding it. */
const offered = computed(() => {
  const typed = draft.value.trim().toLowerCase();
  if (!open.value || !typed) return [];
  const starts: string[] = [];
  const words: string[] = [];
  const holds: string[] = [];
  for (const { name, folded } of names.value) {
    const at = folded.indexOf(typed);
    if (at === -1 || folded === typed) continue;
    if (at === 0) starts.push(name);
    else if (/[^\p{L}\p{N}]/u.test(folded[at - 1])) words.push(name);
    else holds.push(name);
    if (starts.length >= MAX_OFFERED) break;
  }
  return [...starts, ...words, ...holds].slice(0, MAX_OFFERED);
});

function position() {
  const box = input.value?.getBoundingClientRect();
  if (!box) return;
  const height = offered.value.length * OFFER_HEIGHT + 10;
  const above = box.bottom + height > window.innerHeight && box.top - height > 0;
  place.value = {
    left: `${box.left}px`,
    minWidth: `${Math.max(box.width, 160)}px`,
    ...(above ? { bottom: `${window.innerHeight - box.top + 4}px` } : { top: `${box.bottom + 4}px` }),
  };
}

watch(offered, (now) => {
  highlighted.value = -1;
  if (now.length) position();
});

function close() {
  open.value = false;
  highlighted.value = -1;
}

/** The page moving under the list would leave it behind. */
function onScroll(event: Event) {
  if (event.target instanceof Node && input.value?.contains(event.target)) return;
  close();
}
onMounted(() => {
  window.addEventListener('scroll', onScroll, true);
  window.addEventListener('resize', close);
  if (props.autofocus) {
    input.value?.focus();
    input.value?.select();
  }
});

// --- Answering ------------------------------------------------------------------

function commit() {
  if (settled) return;
  settled = true;
  emit('commit', draft.value.trim());
}

function onFocus() {
  focused.value = true;
  settled = false;
  void loadNames();
}

function onBlur() {
  focused.value = false;
  close();
  commit();
  // Whoever shows this box may keep it, with what was saved or what was there.
  void nextTick(() => {
    if (!focused.value) draft.value = props.value;
  });
}

function take(name: string) {
  draft.value = name;
  close();
  input.value?.blur();
}

function onKey(event: KeyboardEvent) {
  // Keys that are still composing a character are the input method's; ones with ⌘ are shortcuts.
  if (event.isComposing || commandKey(event)) return;
  const offers = offered.value;
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    if (props.arrowsLeave && !offers.length) return;
    if (!open.value && suggests.value) open.value = true;
    else if (offers.length) {
      const step = event.key === 'ArrowDown' ? 1 : -1;
      // Past either end is back in the box, with nothing picked.
      highlighted.value = ((highlighted.value + 1 + step + offers.length + 1) % (offers.length + 1)) - 1;
    } else return;
  } else if (event.key === 'Enter') {
    if (highlighted.value >= 0) take(offers[highlighted.value]);
    else input.value?.blur();
  } else if (event.key === 'Tab') {
    if (highlighted.value >= 0) draft.value = offers[highlighted.value];
    return;
  } else if (event.key === 'Escape') {
    // The first Escape puts the list away; the next, the edit.
    if (offers.length) close();
    else {
      draft.value = props.value;
      settled = true;
      emit('cancel');
      input.value?.blur();
    }
  } else return;
  event.preventDefault();
  event.stopPropagation();
}

onBeforeUnmount(() => {
  window.removeEventListener('scroll', onScroll, true);
  window.removeEventListener('resize', close);
  if (focused.value && props.commitOnRemove) commit();
});
</script>

<template>
  <input
    ref="input"
    v-model="draft"
    v-bind="$attrs"
    spellcheck="false"
    autocomplete="off"
    autocorrect="off"
    autocapitalize="off"
    @focus="onFocus"
    @blur="onBlur"
    @input="open = suggests"
    @keydown="onKey"
  />
  <Teleport to="body">
    <ul
      v-if="offered.length"
      class="fixed z-50 max-w-sm overflow-hidden rounded border border-border bg-surface-1 py-1 text-xs text-text-primary shadow-lg"
      :style="place"
    >
      <!-- Pressed, not clicked: a click would take the keyboard from the box first. -->
      <li
        v-for="(name, at) in offered"
        :key="name"
        class="cursor-default truncate px-2"
        :class="at === highlighted ? 'bg-accent/25' : 'hover:bg-surface-2'"
        :style="{ height: `${OFFER_HEIGHT}px`, lineHeight: `${OFFER_HEIGHT}px` }"
        @mousedown.prevent="take(name)"
      >{{ name }}</li>
    </ul>
  </Teleport>
</template>
