<script setup lang="ts">
import { computed, ref } from 'vue';
import FixCard from '@/components/FixCard.vue';
import { useLibraryStore } from '@/stores/library';
import type { LibraryItem } from '@/types/library';

/** What identifying found for the selected tracks, to apply one by one or together. */
const props = defineProps<{ items: LibraryItem[] }>();
defineEmits<{ close: [] }>();

const library = useLibraryStore();
const error = ref<string | null>(null);

const fixes = computed(() => props.items.filter((i) => i.status === 'matched'));
/** Close matches: the ones safe to apply without reading each. */
const sure = computed(() => fixes.value.filter((i) => i.proposal?.confidence === 'high'));

async function applySure() {
  error.value = null;
  try {
    await library.apply(sure.value.map((i) => i.id));
  } catch (e) {
    error.value = String(e);
  }
}
</script>

<template>
  <aside class="flex w-72 shrink-0 flex-col gap-3 overflow-y-auto border-l border-border p-3 text-xs">
    <div class="flex items-center justify-between">
      <span class="section-label">Identified</span>
      <button class="text-text-muted hover:text-text-primary" title="Close" @click="$emit('close')">✕</button>
    </div>
    <button
      v-if="sure.length > 1"
      class="rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
      :disabled="library.busy"
      title="Apply the fixes marked “sure”. The ones marked “check” are left for you to read."
      @click="applySure"
    >
      Apply the {{ sure.length }} sure fixes
    </button>
    <p v-if="error" class="error-box">{{ error }}</p>
    <div v-for="item in items" :key="item.id" class="flex flex-col gap-1">
      <span class="truncate text-text-secondary" :title="item.path">
        {{ item.tags.title ? [item.tags.artist, item.tags.title].filter(Boolean).join(' – ') : item.relativePath }}
      </span>
      <FixCard :item="item" />
    </div>
  </aside>
</template>
