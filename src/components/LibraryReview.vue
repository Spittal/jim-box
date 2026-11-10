<script setup lang="ts">
import { computed, ref } from 'vue';
import LibraryItemRow from '@/components/LibraryItemRow.vue';
import { useLibraryStore } from '@/stores/library';
import type { AlbumProposal, LibraryItem } from '@/types/library';

/** Proposed fixes, grouped by album, to select and apply. */
const props = defineProps<{ items: LibraryItem[]; musicDir: string; busy: boolean }>();
const emit = defineEmits<{ error: [message: string] }>();

const library = useLibraryStore();

// Selection is UI state: high-confidence matches are selected by default and
// `toggled` records the user's deviations from that default.
const toggled = ref(new Set<string>());
function isSelected(item: LibraryItem): boolean {
  const byDefault = item.proposal?.confidence === 'high';
  return byDefault !== toggled.value.has(item.id);
}
function toggle(item: LibraryItem) {
  const next = new Set(toggled.value);
  if (!next.delete(item.id)) next.add(item.id);
  toggled.value = next;
}
function selectAll(selected: boolean) {
  toggled.value = new Set(
    props.items.filter((i) => (i.proposal?.confidence === 'high') !== selected).map((i) => i.id),
  );
}

/** Items grouped by the album they were matched to; others stand alone. */
interface ReviewSection {
  key: string;
  album: AlbumProposal | null;
  items: LibraryItem[];
}
const sections = computed(() => {
  const sections: ReviewSection[] = [];
  const byRelease = new Map<string, ReviewSection>();
  for (const item of props.items) {
    const album = item.proposal?.album ?? null;
    const existing = album ? byRelease.get(album.releaseId) : undefined;
    if (existing) {
      existing.items.push(item);
      continue;
    }
    const section = { key: album?.releaseId ?? item.id, album, items: [item] };
    sections.push(section);
    if (album) byRelease.set(album.releaseId, section);
  }
  return sections;
});

function sectionState(section: ReviewSection): 'all' | 'some' | 'none' {
  const selectable = section.items.filter((i) => i.status === 'matched');
  const count = selectable.filter(isSelected).length;
  return count === 0 ? 'none' : count === selectable.length ? 'all' : 'some';
}
function toggleSection(section: ReviewSection) {
  const select = sectionState(section) !== 'all';
  const next = new Set(toggled.value);
  for (const item of section.items.filter((i) => i.status === 'matched')) {
    if (isSelected(item) !== select) {
      if (!next.delete(item.id)) next.add(item.id);
    }
  }
  toggled.value = next;
}

const selectedIds = computed(() =>
  props.items.filter((i) => i.status === 'matched' && isSelected(i)).map((i) => i.id),
);

async function apply() {
  try {
    await library.apply(selectedIds.value);
  } catch (e) {
    emit('error', String(e));
  }
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <ul class="min-h-0 flex-1 overflow-y-auto">
      <template v-for="section in sections" :key="section.key">
        <li
          v-if="section.album"
          class="flex items-center gap-3 border-b border-border bg-surface-1 px-4 py-2"
        >
          <input
            type="checkbox"
            class="size-3.5 shrink-0 accent-accent"
            :checked="sectionState(section) === 'all'"
            :indeterminate="sectionState(section) === 'some'"
            @change="toggleSection(section)"
          />
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm font-medium">
              {{ section.album.title }}
              <span class="font-normal text-text-muted">
                · {{ section.album.artist }}{{ section.album.date ? ` (${section.album.date.slice(0, 4)})` : '' }}
              </span>
            </div>
            <div class="text-xs text-text-muted">
              Matched as an album: {{ section.album.matched }} of {{ section.album.tracks }} tracks ·
              {{ Math.round((1 - section.album.distance) * 100) }}% match
            </div>
          </div>
        </li>
        <LibraryItemRow
          v-for="item in section.items"
          :key="item.id"
          :item="item"
          :music-dir="musicDir"
          :selectable="true"
          :selected="isSelected(item)"
          @toggle="toggle(item)"
        />
      </template>
      <li v-if="!items.length" class="p-8 text-center text-sm text-text-muted">
        Nothing to review. Identify files to look for fixes.
      </li>
    </ul>

    <footer
      v-if="items.length"
      class="flex items-center justify-between gap-3 border-t border-border px-4 py-2"
    >
      <div class="flex gap-3 text-xs text-text-muted">
        <button class="hover:text-text-secondary" @click="selectAll(true)">Select all</button>
        <button class="hover:text-text-secondary" @click="selectAll(false)">Select none</button>
      </div>
      <button
        :disabled="busy || !selectedIds.length"
        class="rounded bg-accent px-4 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
        @click="apply"
      >
        Apply {{ selectedIds.length }} {{ selectedIds.length === 1 ? 'change' : 'changes' }}
      </button>
    </footer>
  </div>
</template>
