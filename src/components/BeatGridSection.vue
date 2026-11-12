<script setup lang="ts">
/**
 * A track's beat grid, in the track sidebar: how sure the analysis is of it,
 * which of the grids it made the track has, the review that settles it, and
 * analysing again with what the user knows of the tempo.
 *
 * Read top to bottom it is the order of the work: see what was found, try the
 * other grids against the music, mark the right one reviewed; or, if none is
 * right, analyse again with a hint.
 */
import { computed, ref, watch } from 'vue';
import { GRID_SOURCES, gridChoiceTitle, gridClass, gridFollowsTempo, gridIssues } from '@/composables/useGridReport';
import { useCollectionStore } from '@/stores/collection';
import type { CollectionTrack, GridHint, GridSource } from '@/types/collection';

const props = defineProps<{ track: CollectionTrack }>();

const store = useCollectionStore();
const error = ref<string | null>(null);

const grid = computed(() => props.track.grid);
const report = computed(() => grid.value?.report ?? null);
const analysing = computed(() => store.analysis?.running.includes(props.track.id) ?? false);
const queued = computed(() => !analysing.value && !!store.analysis?.active && waiting.value);
/** Set when this section queues the track, until it's analysed. */
const waiting = ref(false);
watch(analysing, (now, was) => {
  if (was && !now) waiting.value = false;
});
const analysisError = computed(
  () => store.analysis?.failed.find((f) => f.trackId === props.track.id)?.error ?? null,
);

/** The grid the track has now, in a few words. */
const inUse = computed(() => {
  if (grid.value?.using === 'hand') return 'set by hand';
  return grid.value && gridFollowsTempo(grid.value) ? 'follows the tempo' : 'one tempo';
});

async function attempt(action: () => Promise<unknown>) {
  error.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

/** Tries another of the grids the analysis made; the player draws it straight away. */
function use(source: GridSource) {
  attempt(async () => {
    const done = await store.edit({ kind: 'useGrid', trackId: props.track.id, source });
    if (done === undefined) throw store.error;
  });
}

function review(reviewed: boolean) {
  attempt(() => store.reviewGrid(props.track.id, reviewed));
}

// --- Analysing again, with a hint --------------------------------------------

const again = ref(false);
/** The tempo hint as typed; empty for none. */
const hintBpm = ref('');
/** Whether the tempo moves: null leaves it to the analysis. */
const hintVaries = ref<boolean | null>(null);

/** Opens with what the last analysis was told, to adjust rather than retype. */
function openAgain() {
  hintBpm.value = grid.value?.hint.bpm ? String(grid.value.hint.bpm) : '';
  hintVaries.value = grid.value?.hint.varies ?? null;
  taps.value = [];
  again.value = true;
}
// Another track: its own hint, and nothing half-filled-in.
watch(
  () => props.track.id,
  () => {
    again.value = false;
    waiting.value = false;
    error.value = null;
  },
);

const hint = computed<GridHint | null | 'invalid'>(() => {
  const typed = hintBpm.value.trim();
  const bpm = typed ? Number(typed) : null;
  if (bpm !== null && !(bpm >= 40 && bpm <= 250)) return 'invalid';
  return bpm === null && hintVaries.value === null ? null : { bpm, varies: hintVaries.value };
});

/** Tap along to fill the tempo in: four taps or more, forgotten after a pause. */
const taps = ref<number[]>([]);
function tap() {
  const now = performance.now();
  const last = taps.value[taps.value.length - 1];
  taps.value = [...(last !== undefined && now - last > 2500 ? [] : taps.value), now];
  if (taps.value.length >= 4) {
    const span = taps.value[taps.value.length - 1] - taps.value[0];
    hintBpm.value = (60000 / (span / (taps.value.length - 1))).toFixed(1);
  }
}

const VARIES: { value: boolean | null; label: string; title: string }[] = [
  { value: null, label: 'Auto', title: 'Let the analysis decide whether the grid follows the tempo' },
  { value: true, label: 'Moves', title: 'A drummer who drifts, or a change of pace: the grid follows the tempo whatever the audio suggests' },
  { value: false, label: 'Steady', title: 'Made on a grid: one tempo for the whole track' },
];

async function analyse() {
  if (hint.value === 'invalid') return;
  const told = hint.value;
  again.value = false;
  waiting.value = true;
  error.value = null;
  const queuedCount = await store.analyse([props.track.id], true, told);
  if (queuedCount === undefined) {
    waiting.value = false;
    error.value = store.error;
  }
}

/** What the last analysis was told, as a sentence; null when nothing. */
const told = computed(() => {
  const given = grid.value?.hint;
  if (!given || (given.bpm === null && given.varies === null)) return null;
  const parts = [
    given.bpm !== null ? `it's about ${given.bpm} BPM` : null,
    given.varies === true ? 'its tempo moves' : given.varies === false ? 'its tempo is steady' : null,
  ].filter(Boolean);
  return `Analysed knowing ${parts.join(' and ')}.`;
});

const quiet = 'rounded bg-surface-2 px-2 py-1 text-[11px] transition-colors hover:bg-surface-3 disabled:opacity-50';
const primary =
  'rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50';
const segment = 'flex-1 px-2 py-1 text-[11px] transition-colors';
</script>

<template>
  <section class="flex flex-col gap-2.5 border-t border-border pt-3">
    <!-- What it is, and how sure. -->
    <header class="flex items-baseline justify-between gap-2">
      <span class="section-label">Beat grid</span>
      <span v-if="analysing" class="text-[11px] text-status-busy">Analysing…</span>
      <span v-else-if="queued" class="text-[11px] text-text-muted">Queued</span>
      <span v-else-if="!track.analysed" class="text-[11px] text-text-muted">Not analysed</span>
      <span v-else-if="grid?.reviewed" class="text-[11px] font-medium" :class="gridClass(grid)">✓ Reviewed</span>
      <span v-else-if="grid && report" class="text-[11px]" :class="gridClass(grid)">
        <span class="font-mono font-medium">{{ report.confidence }}%</span> sure
      </span>
      <span v-else class="text-[11px] text-text-muted">Not reviewed</span>
    </header>

    <template v-if="track.analysed && !analysing">
      <p class="flex items-baseline gap-2">
        <span class="font-mono text-base leading-none">{{ track.bpm?.toFixed(2) ?? '–' }}</span>
        <span class="text-[11px] text-text-muted">BPM · {{ inUse }}</span>
      </p>

      <!-- Why it isn't surer: gone once the user has settled it. -->
      <ul
        v-if="report?.issues.length && !grid?.reviewed"
        class="flex flex-col gap-1 border-l-2 pl-2 text-[11px] text-text-secondary"
        :class="report.confidence >= 60 ? 'border-status-busy/60' : 'border-status-error/60'"
      >
        <li v-for="issue in gridIssues(report)" :key="issue">{{ issue }}</li>
      </ul>
      <p v-if="told" class="text-[11px] text-text-muted">{{ told }}</p>

      <!-- The grids to try. -->
      <div v-if="grid && grid.choices.length > 1" class="flex flex-col gap-1">
        <span class="text-[11px] text-text-muted">Grid in use</span>
        <div class="flex overflow-hidden rounded border border-border">
          <button
            v-for="choice in grid.choices"
            :key="choice.source"
            :class="[
              segment,
              'flex flex-col items-center border-l border-border first:border-l-0',
              grid.using === choice.source ? 'bg-accent/20 text-text-primary' : 'bg-surface-1 text-text-secondary hover:bg-surface-2',
            ]"
            :title="gridChoiceTitle(choice)"
            :disabled="grid.using === choice.source"
            @click="use(choice.source)"
          >
            <span>{{ GRID_SOURCES[choice.source] }}</span>
            <span class="font-mono text-[10px] text-text-muted">{{ choice.bpm.toFixed(1) }}{{ choice.dynamic ? '~' : '' }}</span>
          </button>
        </div>
        <span v-if="grid.using === 'hand'" class="text-[11px] text-text-muted">
          Changed by hand since; pick one to go back to it.
        </span>
        <span v-else class="text-[11px] text-text-muted">Pick one and listen in the player.</span>
      </div>

      <!-- Settling it. -->
      <div v-if="!again" class="flex items-center gap-2">
        <button
          v-if="!grid?.reviewed"
          :class="primary"
          title="You've checked this grid against the music and it's right. Analysing a batch of tracks again leaves reviewed grids alone."
          @click="review(true)"
        >
          Mark as reviewed
        </button>
        <button
          v-else
          :class="quiet"
          title="Take the review back: the Grid column shows how sure the analysis was again"
          @click="review(false)"
        >
          Undo review
        </button>
        <button :class="quiet" title="Work the grid out again, with a tempo hint if the analysis got it wrong" @click="openAgain">
          Analyse again…
        </button>
      </div>
    </template>

    <button v-else-if="!analysing && !queued && !again" :class="[primary, 'self-start']" @click="openAgain">
      Analyse…
    </button>

    <!-- Analysing, with what the user knows of the tempo. -->
    <div v-if="again && !analysing" class="flex flex-col gap-2 rounded border border-border bg-surface-1 p-2">
      <label class="flex flex-col gap-1">
        <span class="text-[11px] text-text-muted">Tempo hint (optional)</span>
        <span class="flex items-center gap-1.5">
          <input
            v-model="hintBpm"
            inputmode="decimal"
            placeholder="e.g. 125"
            class="w-20 rounded border border-border bg-surface-0 px-2 py-1 font-mono text-xs outline-none focus:border-border-active"
            :class="hint === 'invalid' ? 'border-status-error' : ''"
            @keydown.enter="analyse"
          />
          <span class="text-[11px] text-text-muted">BPM</span>
          <button
            type="button"
            :class="[quiet, 'ml-auto']"
            title="Tap along with the music, four times or more, to fill the tempo in"
            @click.prevent="tap"
          >
            Tap{{ taps.length ? ` ${taps.length}` : '' }}
          </button>
        </span>
        <span v-if="hint === 'invalid'" class="text-[11px] text-status-error">Between 40 and 250.</span>
        <span v-else class="text-[11px] text-text-muted">Roughly is enough: it settles 77 against 154, and keeps the analysis near it.</span>
      </label>
      <div class="flex flex-col gap-1">
        <span class="text-[11px] text-text-muted">Its tempo</span>
        <div class="flex overflow-hidden rounded border border-border">
          <button
            v-for="option in VARIES"
            :key="String(option.value)"
            type="button"
            :class="[
              segment,
              'border-l border-border first:border-l-0',
              hintVaries === option.value ? 'bg-accent/20 text-text-primary' : 'bg-surface-0 text-text-secondary hover:bg-surface-2',
            ]"
            :title="option.title"
            @click="hintVaries = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>
      <p v-if="track.analysed" class="text-[11px]" :class="grid?.reviewed ? 'text-status-busy' : 'text-text-muted'">
        {{ grid?.reviewed ? 'Replaces the grid you reviewed.' : 'Replaces the beat grid.' }} Cues are kept.
      </p>
      <div class="flex items-center gap-2">
        <button :class="primary" :disabled="hint === 'invalid'" @click="analyse">
          {{ track.analysed ? 'Analyse again' : 'Analyse' }}
        </button>
        <button :class="quiet" @click="again = false">Cancel</button>
      </div>
    </div>

    <p v-if="analysisError" class="error-box">{{ analysisError }}</p>
    <p v-if="error" class="error-box">{{ error }}</p>
  </section>
</template>
