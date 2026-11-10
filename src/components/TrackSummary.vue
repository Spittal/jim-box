<script setup lang="ts">
import CoverArt from '@/components/CoverArt.vue';
import { formatDuration } from '@/composables/useFormat';
import type { TrackRequest } from '@/types/track';

/** A search result's cover, title and "artist · album · length" line. */
defineProps<{ track: TrackRequest; coverClass?: string }>();
</script>

<template>
  <div class="flex min-w-0 flex-1 items-center gap-3">
    <CoverArt :src="track.coverUrl" :class="coverClass ?? 'size-10'" />
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-1.5">
        <span class="truncate text-sm">{{ track.title }}</span>
        <span
          v-if="track.deezerId === null"
          class="shrink-0 rounded bg-surface-2 px-1.5 font-mono text-[10px] leading-4 text-text-muted"
          title="From MusicBrainz: not on Deezer, or a different version"
        >MusicBrainz</span>
      </div>
      <div class="truncate text-xs text-text-muted">
        {{ track.artist }}<template v-if="track.album"> · {{ track.album }}</template>
        · {{ formatDuration(track.durationSecs) }}
      </div>
    </div>
  </div>
</template>
