<script setup lang="ts">
import { storeToRefs } from 'pinia';
import PlaylistEntryPicker from '@/components/PlaylistEntryPicker.vue';
import { useBrowseStore } from '@/stores/browse';

/**
 * A Spotify playlist's track that isn't in the collection and has no fetch
 * yet: picking its match. One being fetched shows in the Downloads window.
 */
const browse = useBrowseStore();
const { focused } = storeToRefs(browse);
</script>

<template>
  <aside v-if="focused" class="w-[26rem] shrink-0 overflow-y-auto border-l border-border">
    <div class="sticky top-0 z-10 flex justify-end bg-surface-0 px-2 pt-2">
      <button
        class="rounded px-2 py-0.5 text-sm text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
        title="Close"
        @click="browse.clearSelection()"
      >
        ✕
      </button>
    </div>
    <PlaylistEntryPicker :key="focused.index" :entry="focused" />
  </aside>
</template>
