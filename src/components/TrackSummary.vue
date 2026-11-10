<script setup lang="ts">
import { computed, onBeforeUnmount } from 'vue';
import CoverArt from '@/components/CoverArt.vue';
import { canPreview, preview, stopPreview, togglePreview } from '@/audio/preview';
import { formatDuration } from '@/composables/useFormat';
import { trackKey, type TrackRequest } from '@/types/track';

/**
 * A search result's cover, title and "artist · album · length" line. Clicking
 * the cover plays Deezer's 30-second clip of the track, if it has one.
 */
const props = defineProps<{ track: TrackRequest; coverClass?: string }>();

const key = computed(() => trackKey(props.track));
const previewable = computed(() => canPreview(props.track));
const active = computed(() => preview.key === key.value);
const failed = computed(() => preview.failedKey === key.value);

// The clip belongs to the row: it stops when the results go.
onBeforeUnmount(() => active.value && stopPreview());
</script>

<template>
  <div class="flex min-w-0 flex-1 items-center gap-3">
    <button
      v-if="previewable"
      type="button"
      class="group relative shrink-0 overflow-hidden rounded"
      :class="coverClass ?? 'size-10'"
      :title="failed ? 'The preview couldn\'t be played' : active ? 'Stop the preview' : 'Play a 30-second preview'"
      :aria-label="active ? 'Stop the preview' : 'Play a 30-second preview'"
      @click.stop="togglePreview(track)"
    >
      <CoverArt :src="track.coverUrl" class="size-full" />
      <span
        class="absolute inset-0 flex items-center justify-center bg-black/50 text-white transition-opacity"
        :class="active ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100'"
      >
        <span v-if="active && preview.loading" class="size-3.5 animate-spin rounded-full border-2 border-white/40 border-t-white" />
        <svg v-else viewBox="0 0 16 16" class="size-4" fill="currentColor" aria-hidden="true">
          <path v-if="active" d="M4 3h3v10H4zM9 3h3v10H9z" />
          <path v-else d="M5 3l8 5-8 5z" />
        </svg>
      </span>
      <span
        v-if="active"
        class="absolute bottom-0 left-0 h-0.5 bg-accent"
        :style="{ width: `${preview.progress * 100}%` }"
      />
    </button>
    <CoverArt v-else :src="track.coverUrl" :class="coverClass ?? 'size-10'" />
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
