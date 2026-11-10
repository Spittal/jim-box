<script setup lang="ts">
import { computed, ref } from 'vue';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import FixCard from '@/components/FixCard.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import LibraryCover from '@/components/LibraryCover.vue';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { formatDuration } from '@/composables/useFormat';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import type { CollectionTrack, TrackField } from '@/types/collection';

const props = defineProps<{ track: CollectionTrack }>();
defineEmits<{ close: [] }>();

const store = useCollectionStore();
const libraryStore = useLibraryStore();
const jobsStore = useJobsStore();
const browse = useBrowseStore();

const actionError = ref<string | null>(null);
const starting = ref(false);

const lossless = computed(() => ['flac', 'wav', 'aiff'].includes(props.track.format));
/** The latest download that replaces, or produced, this track's file. */
const job = computed(() =>
  jobsStore.jobs.find((j) => j.replaces === props.track.path || j.outputPath === props.track.path),
);
const downloading = computed(() => !!job.value && isActive(job.value));
/** What identifying found for this track, if it has been looked up. */
const identified = computed(() => libraryStore.forTrack(props.track.id) ?? null);

async function run(action: () => Promise<unknown>) {
  actionError.value = null;
  try {
    await action();
  } catch (e) {
    actionError.value = String(e);
  }
}

function redownload() {
  starting.value = true;
  run(() => libraryStore.redownload(props.track.id)).finally(() => (starting.value = false));
}

const analysing = computed(() => store.analysis?.running.includes(props.track.id) ?? false);
const analysisError = computed(
  () => store.analysis?.failed.find((f) => f.trackId === props.track.id)?.error ?? null,
);
/** Asked for before replacing an analysis: the beat grid may have been adjusted by hand. */
const confirmingAnalysis = ref(false);

async function analyse() {
  confirmingAnalysis.value = false;
  await store.analyse([props.track.id], true);
}

const actionClass =
  'rounded bg-surface-2 px-2 py-0.5 text-[11px] transition-colors hover:bg-surface-3 disabled:opacity-50';

const fields: { id: TrackField; label: string }[] = [
  { id: 'title', label: 'Title' },
  { id: 'artist', label: 'Artist' },
  { id: 'album', label: 'Album' },
  { id: 'genre', label: 'Genre' },
];

/** Saves a field when its box is left or Enter is pressed, if it changed. */
function commitField(field: TrackField, event: Event) {
  const input = event.target as HTMLInputElement;
  const value = input.value.trim();
  if (value === props.track[field as 'title' | 'artist' | 'album' | 'genre']) return;
  browse.patchTrack(props.track.id, { [field]: value });
  store.edit({ kind: 'setField', trackId: props.track.id, field, value });
}

function commitComment(event: Event) {
  const comment = (event.target as HTMLTextAreaElement).value;
  if (comment === props.track.comment) return;
  browse.patchTrack(props.track.id, { comment });
  store.edit({ kind: 'setComment', trackId: props.track.id, comment });
}

/** Clicking the current rating clears it, as in rekordbox. */
function rate(stars: number) {
  const next = props.track.rating === stars ? 0 : stars;
  // Shown at once: saving takes a moment, and a star should answer the click.
  browse.patchTrack(props.track.id, { rating: next });
  store.edit({ kind: 'setRating', trackId: props.track.id, stars: next });
}

function colour(color: number) {
  browse.patchTrack(props.track.id, { color });
  store.edit({ kind: 'setColor', trackId: props.track.id, color });
}

const inputClass =
  'w-full rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none focus:border-border-active';
</script>

<template>
  <aside class="flex w-64 shrink-0 flex-col gap-3 overflow-y-auto border-l border-border p-3 text-xs">
    <div class="flex items-center justify-between">
      <span class="section-label">Track</span>
      <button class="text-text-muted hover:text-text-primary" title="Close" @click="$emit('close')">✕</button>
    </div>

    <LibraryCover :key="track.path" :path="track.path" class="aspect-square w-24 self-center" />

    <FixCard v-if="identified" :item="identified" />

    <!-- Keyed by track so the boxes reset when another track is picked. -->
    <label v-for="field in fields" :key="`${track.id}-${field.id}`" class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">{{ field.label }}</span>
      <input
        :value="track[field.id as 'title']"
        :class="inputClass"
        spellcheck="false"
        @change="commitField(field.id, $event)"
        @keydown.enter="($event.target as HTMLInputElement).blur()"
      />
    </label>

    <label class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">Comment</span>
      <textarea
        :key="track.id"
        :value="track.comment"
        :class="inputClass"
        rows="3"
        @change="commitComment"
      />
    </label>

    <div class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">Rating</span>
      <div class="flex gap-0.5 text-base leading-none">
        <button
          v-for="star in 5"
          :key="star"
          :class="star <= track.rating ? 'text-text-primary' : 'text-text-muted/40 hover:text-text-muted'"
          :title="`${star} ${star === 1 ? 'star' : 'stars'}`"
          @click="rate(star)"
        >★</button>
      </div>
    </div>

    <div class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">Colour</span>
      <div class="flex items-center gap-1.5">
        <button
          v-for="(color, id) in TRACK_COLORS"
          :key="id"
          class="size-4 rounded-full border"
          :class="track.color === id ? 'border-text-primary' : 'border-border'"
          :style="{ backgroundColor: color.hex || 'transparent' }"
          :title="color.name"
          @click="colour(id)"
        />
      </div>
    </div>

    <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[11px]">
      <dt class="text-text-muted">BPM</dt>
      <dd class="font-mono">{{ track.bpm?.toFixed(2) ?? '–' }}</dd>
      <dt class="text-text-muted">Key</dt>
      <dd class="font-mono">{{ track.key || '–' }}</dd>
      <dt class="text-text-muted">Time</dt>
      <dd class="font-mono">{{ track.durationSecs ? formatDuration(track.durationSecs) : '–' }}</dd>
      <dt class="text-text-muted">Format</dt>
      <dd class="font-mono">{{ track.format.toUpperCase() }} {{ track.bitrate ? `${track.bitrate}k` : '' }}</dd>
      <dt class="text-text-muted">Analysed</dt>
      <dd>{{ analysing ? 'Analysing…' : track.analysed ? 'Yes' : 'Not yet' }}</dd>
    </dl>

    <div class="flex flex-col gap-1">
      <div class="flex flex-wrap gap-1.5">
        <template v-if="confirmingAnalysis">
          <button :class="actionClass" @click="analyse">Replace its beat grid</button>
          <button :class="actionClass" @click="confirmingAnalysis = false">Cancel</button>
        </template>
        <button
          v-else
          :class="actionClass"
          :disabled="analysing"
          :title="track.analysed
            ? 'Work out the BPM, beat grid, key and waveforms again. Cues are kept; the beat grid is replaced.'
            : 'Work out the BPM, beat grid, key and waveforms'"
          @click="track.analysed ? (confirmingAnalysis = true) : analyse()"
        >
          {{ track.analysed ? 'Analyse again…' : 'Analyse' }}
        </button>
      </div>
      <p v-if="analysisError" class="error-box">{{ analysisError }}</p>
    </div>

    <div class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">File</span>
      <p class="font-mono text-[10px] break-all text-text-secondary">{{ track.path }}</p>
      <div class="flex flex-wrap gap-1.5">
        <button :class="actionClass" @click="run(() => revealItemInDir(track.path))">Show in Finder</button>
        <button
          :class="actionClass"
          :disabled="libraryStore.busy"
          title="Look this track up on MusicBrainz and propose the tags and file name it should have, to review here"
          @click="run(() => libraryStore.identify([track.id]))"
        >
          Identify
        </button>
        <button
          :class="actionClass"
          :disabled="downloading || starting"
          :title="lossless
            ? 'Download this track again. It keeps its cues and playlists; the old file goes to the Trash.'
            : 'Download a lossless copy. The track keeps its cues and playlists; the old file goes to the Trash.'"
          @click="redownload"
        >
          {{ lossless ? 'Redownload' : 'Get lossless' }}
        </button>
      </div>
      <button
        v-if="job"
        class="inline-flex self-start text-[11px] text-text-secondary hover:text-text-primary"
        title="Show where this file came from, and the other sources found for it"
        @click="browse.focus = { kind: 'job', id: job.id }"
      >
        <JobStatusBadge :job="job" />
      </button>
      <p v-if="actionError" class="error-box">{{ actionError }}</p>
    </div>
  </aside>
</template>
