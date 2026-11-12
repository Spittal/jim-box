<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import BeatGridSection from '@/components/BeatGridSection.vue';
import FieldInput from '@/components/FieldInput.vue';
import FixCard from '@/components/FixCard.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import LibraryCover from '@/components/LibraryCover.vue';
import PlaylistPills from '@/components/PlaylistPills.vue';
import { TRACK_COLORS } from '@/composables/useTrackColors';
import { formatDuration } from '@/composables/useFormat';
import { formatKey as keyIn, useKeyNotation } from '@/composables/useKeyNotation';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { revealLabel } from '@/platform';
import type { CollectionTrack, TrackField } from '@/types/collection';

const props = defineProps<{ track: CollectionTrack }>();
defineEmits<{ close: [] }>();

const store = useCollectionStore();
const { formatKey, allKeys } = useKeyNotation();
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

const actionClass =
  'rounded bg-surface-2 px-2 py-0.5 text-[11px] transition-colors hover:bg-surface-3 disabled:opacity-50';

const fields: { id: TrackField; label: string }[] = [
  { id: 'title', label: 'Title' },
  { id: 'artist', label: 'Artist' },
  { id: 'album', label: 'Album' },
  { id: 'genre', label: 'Genre' },
];

/** Saves a field when its box is left or Return is pressed, if it changed. */
function commitField(field: TrackField, value: string) {
  if (value === props.track[field as 'title' | 'artist' | 'album' | 'genre']) return;
  browse.patchTrack(props.track.id, { [field]: value });
  store.edit({ kind: 'setField', trackId: props.track.id, field, value });
}

// --- Genres from MusicBrainz ----------------------------------------------------

/** How long a track is shown before MusicBrainz is asked about it, so passing over tracks asks nothing. */
const GENRE_DWELL_MS = 600;

/** What MusicBrainz has for this track's genre, the likeliest first; null until it has answered. */
const suggested = ref<string[] | null>(null);
const suggestError = ref<string | null>(null);
let dwell: ReturnType<typeof setTimeout> | undefined;

/** A genre as the collection already spells it, or with its words capitalised: MusicBrainz's are in lower case. */
async function spelled(genre: string): Promise<string> {
  const names = await store.namesOf('genre');
  const folded = genre.toLowerCase();
  return (
    names.find((name) => name.toLowerCase() === folded) ??
    genre.replace(/(^|[\s-])\p{L}/gu, (start) => start.toUpperCase())
  );
}

watch(
  // Identifying a track says which recording it is, which is worth asking again for.
  () => [props.track.id, identified.value?.proposal?.metadata.mbRecordingId] as const,
  ([trackId]) => {
    clearTimeout(dwell);
    suggested.value = null;
    suggestError.value = null;
    dwell = setTimeout(async () => {
      try {
        const found = await Promise.all((await libraryStore.genres(trackId)).map(spelled));
        if (props.track.id === trackId) suggested.value = [...new Set(found)];
      } catch (e) {
        if (props.track.id === trackId) suggestError.value = String(e);
      }
    }, GENRE_DWELL_MS);
  },
  { immediate: true },
);
onBeforeUnmount(() => clearTimeout(dwell));

/** The track's key as rekordbox spells it, which is how the menu's keys are named. */
const keyName = computed(() => keyIn(props.track.key, 'standard'));
/** The keys to choose from. One the track has that isn't a key is kept on the menu, so it shows. */
const keyChoices = computed(() => {
  const keys = allKeys();
  return !keyName.value || keys.includes(keyName.value) ? keys : [keyName.value, ...keys];
});

function chooseKey(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (value === keyName.value) return;
  browse.patchTrack(props.track.id, { key: value });
  store.edit({ kind: 'setField', trackId: props.track.id, field: 'key', value });
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
    <template v-for="field in fields" :key="`${track.id}-${field.id}`">
      <label class="flex flex-col gap-1">
        <span class="text-[11px] text-text-muted">{{ field.label }}</span>
        <FieldInput
          :field="field.id"
          :value="track[field.id as 'title']"
          :class="inputClass"
          @commit="commitField(field.id, $event)"
        />
      </label>
      <!-- What MusicBrainz calls it, each a button that makes it the genre. -->
      <div v-if="field.id === 'genre'" class="-mt-1.5 flex flex-wrap items-center gap-1 text-[11px] text-text-muted">
        <span title="Genres MusicBrainz's users have given this recording, its album or its artist">MusicBrainz:</span>
        <span v-if="suggestError" :title="suggestError">couldn't be asked</span>
        <span v-else-if="!suggested">looking…</span>
        <span v-else-if="!suggested.length">no genre</span>
        <template v-else>
          <button
            v-for="genre in suggested"
            :key="genre"
            class="rounded px-1.5 py-0.5 transition-colors"
            :class="genre === track.genre
              ? 'bg-accent/25 text-text-primary'
              : 'bg-surface-2 text-text-secondary hover:bg-surface-3 hover:text-text-primary'"
            :disabled="genre === track.genre"
            :title="genre === track.genre ? 'The genre it has' : `Set the genre to ${genre}`"
            @click="commitField('genre', genre)"
          >{{ genre }}</button>
        </template>
      </div>
    </template>

    <label class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">Key</span>
      <select :value="keyName" :class="[inputClass, 'font-mono']" @change="chooseKey">
        <option value="">–</option>
        <option v-for="key in keyChoices" :key="key" :value="key">{{ formatKey(key) }}</option>
      </select>
    </label>

    <div class="flex flex-col gap-1">
      <span class="text-[11px] text-text-muted">Playlists</span>
      <PlaylistPills :track="track" />
    </div>

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
      <dt class="text-text-muted">Time</dt>
      <dd class="font-mono">{{ track.durationSecs ? formatDuration(track.durationSecs) : '–' }}</dd>
      <dt class="text-text-muted">Format</dt>
      <dd class="font-mono">{{ track.format.toUpperCase() }} {{ track.bitrate ? `${track.bitrate}k` : '' }}</dd>
    </dl>

    <BeatGridSection :track="track" />

    <div class="flex flex-col gap-1.5 border-t border-border pt-3">
      <span class="section-label">File</span>
      <p class="font-mono text-[10px] break-all text-text-secondary">{{ track.path }}</p>
      <div class="flex flex-wrap gap-1.5">
        <button :class="actionClass" @click="run(() => revealItemInDir(track.path))">{{ revealLabel }}</button>
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
