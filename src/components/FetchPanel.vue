<script setup lang="ts">
import { storeToRefs } from 'pinia';
import JobDetail from '@/components/JobDetail.vue';
import PlaylistEntryPicker from '@/components/PlaylistEntryPicker.vue';
import { useBrowseStore } from '@/stores/browse';

/**
 * A track that isn't in the collection yet: how its fetch is going and the
 * sources found for it, or, for a Spotify track with none, picking its match.
 */
const browse = useBrowseStore();
const { focused, focus } = storeToRefs(browse);
</script>

<template>
  <aside v-if="focused" class="w-[26rem] shrink-0 overflow-y-auto border-l border-border">
    <div class="sticky top-0 z-10 flex justify-end bg-surface-0 px-2 pt-2">
      <button
        class="rounded px-2 py-0.5 text-sm text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
        title="Close"
        @click="focus = null"
      >
        ✕
      </button>
    </div>
    <JobDetail v-if="focused.job" :key="focused.job.id" :job="focused.job" @removed="focus = null" />
    <PlaylistEntryPicker v-else-if="focused.entry" :key="focused.entry.index" :entry="focused.entry" />
  </aside>
</template>
