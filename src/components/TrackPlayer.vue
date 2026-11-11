<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { preview, stopPreview } from '@/audio/preview';
import { TrackAudio } from '@/audio/trackAudio';
import {
  PALETTES,
  chosenPalette,
  cueColor,
  cueLabel,
  drawSlice,
  fit,
  head,
  type Wave,
} from '@/composables/useWaveform';
import { useKeyNotation } from '@/composables/useKeyNotation';
import { useCollectionStore } from '@/stores/collection';
import type { Beat, CollectionEdit, CollectionTrack, Cue, GridChange, Preparation } from '@/types/collection';

/**
 * The player, across the top of the collection as in rekordbox: plays the
 * track loaded into it and prepares it — hot cues, memory cues, loops and the
 * beat grid — over rekordbox's own waveforms, drawn the way rekordbox draws
 * them.
 */
/** The loaded track; null shows the player empty, waiting for one. */
const props = defineProps<{ track: CollectionTrack | null }>();
const emit = defineEmits<{ close: []; ended: [] }>();

const store = useCollectionStore();
const { formatKey } = useKeyNotation();

/** rekordbox's detailed waveforms have 150 columns a second. */
const COLUMNS_PER_SEC = 150;
const PADS = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'];
const LOOP_LENGTHS = [1, 2, 4, 8, 16, 32];

// rekordbox's own colours, as rbxport measured them from it.
const COLOR = {
  beat: '#4c4c4c',
  beatHead: '#7c7c7c',
  downbeat: '#ffffff',
  downbeatHead: '#ea3323',
  playheadPaused: '#ff0000',
  playheadPlaying: '#ffffff',
};
/** The track's audio: see `TrackAudio` for why it isn't an <audio> element. */
const engine = new TrackAudio();
/** The audio can be played and sought. */
const ready = ref(false);
/** The audio's own length, once it's known. */
const audioMs = ref(0);
const loading = ref(false);
const error = ref<string | null>(null);
const playing = ref(false);
/** The playhead, in milliseconds. */
const position = ref(0);

const prep = ref<Preparation | null>(null);
/** The waveforms the track's analysis holds, by kind: 7, 5 and 3. */
const waves = shallowRef(new Map<number, Uint8Array>());

/** The chosen style, or the nearest one this track has a waveform for. */
const palette = computed(() => {
  const has = (p: (typeof PALETTES)[number]) => waves.value.has(p.kind);
  return PALETTES.find((p) => p.id === chosenPalette.value && has(p)) ?? PALETTES.find(has) ?? null;
});
const columns = computed(() => {
  const p = palette.value;
  const data = p && waves.value.get(p.kind);
  return data ? Math.floor(data.length / (p.kind === 7 ? 3 : p.kind === 5 ? 2 : 1)) : 0;
});

const durationMs = computed(() => {
  return audioMs.value || prep.value?.durationMs || (props.track?.durationSecs ?? 0) * 1000;
});
const beats = computed(() => prep.value?.beats ?? []);
/** Each beat's bar, counted from the first bar start. */
const bars = computed(() => {
  let bar = 0;
  return beats.value.map((beat) => (beat[1] === 1 ? ++bar : bar));
});
const cues = computed(() => prep.value?.cues ?? []);
const hotCues = computed(() => PADS.map((_, slot) => cues.value.find((c) => c.slot === slot) ?? null));

// --- Loading -----------------------------------------------------------------

/** Identifies the latest load, so an earlier one's late answer is dropped. */
let latestLoad = 0;

/**
 * Reads the track's beat grid and cues, and its waveforms too unless only
 * the grid or cues changed: the waveforms are hundreds of kilobytes.
 */
async function loadPreparation(withWaveform = true) {
  const id = props.track?.id;
  if (!id) return;
  const load = ++latestLoad;
  try {
    const [nextPrep, bytes] = await Promise.all([
      store.preparation(id),
      withWaveform ? store.waveform(id) : null,
    ]);
    if (props.track?.id !== id || load !== latestLoad) return;
    prep.value = nextPrep;
    if (!bytes) return;
    const found = new Map<number, Uint8Array>();
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    for (let at = 0; at + 5 <= bytes.length; ) {
      const length = view.getUint32(at + 1, true);
      found.set(bytes[at], bytes.subarray(at + 5, at + 5 + length));
      at += 5 + length;
    }
    waves.value = found;
  } catch (e) {
    error.value = String(e);
  }
}

async function loadTrack() {
  const id = props.track?.id;
  engine.unload();
  ready.value = false;
  audioMs.value = 0;
  playing.value = false;
  position.value = 0;
  activeLoop.value = null;
  prep.value = null;
  waves.value = new Map();
  error.value = null;
  undoStack.value = [];
  taps.value = [];
  loading.value = false;
  if (!id) return;
  loading.value = true;
  loadPreparation();
  try {
    // Plays as soon as its first samples arrive: the player was opened to listen.
    await engine.load(id);
  } catch (e) {
    if (props.track?.id === id) {
      error.value = String(e);
      loading.value = false;
    }
  }
}

engine.onstate = () => {
  // One thing plays at a time: the track takes over from a search preview.
  if (engine.playing) stopPreview();
  ready.value = engine.ready;
  if (engine.ready) loading.value = false;
  audioMs.value = engine.durationMs;
  playing.value = engine.playing;
  // Paused, the audio says exactly where; playing, each frame asks.
  if (engine.ready && !engine.playing) position.value = engine.positionMs();
};
// ...and a search preview takes over from the track.
watch(
  () => preview.key,
  (key) => key && engine.playing && engine.pause(),
);
engine.onended = () => emit('ended');
engine.onerror = (e) => {
  error.value = `The audio couldn't be played: ${e}`;
  loading.value = false;
};

// --- Transport ---------------------------------------------------------------

function toggle() {
  if (engine.playing) engine.pause();
  else engine.play();
}

function seek(ms: number) {
  const clamped = Math.max(0, Math.min(ms, durationMs.value));
  position.value = clamped;
  engine.seek(clamped);
}

/** The index of the beat nearest a time, or -1 without a grid. */
function nearestBeat(ms: number): number {
  const list = beats.value;
  if (!list.length) return -1;
  let lo = 0;
  let hi = list.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (list[mid][0] < ms) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 && ms - list[lo - 1][0] <= list[lo][0] - ms ? lo - 1 : lo;
}

function jumpBeats(count: number) {
  const at = nearestBeat(position.value);
  if (at < 0) return seek(position.value + count * 500);
  const list = beats.value;
  // From between beats, one step back goes to the beat just passed.
  const onBeat = Math.abs(list[at][0] - position.value) < 15;
  let target = at + count;
  if (!onBeat && count < 0 && list[at][0] < position.value) target = at + count + 1;
  if (!onBeat && count > 0 && list[at][0] > position.value) target = at + count - 1;
  seek(list[Math.max(0, Math.min(target, list.length - 1))][0]);
}

/** A loop being played round: reaching its end goes back to its start. */
const activeLoop = ref<{ startMs: number; endMs: number } | null>(null);

function goToCue(cue: Cue) {
  activeLoop.value = cue.endMs ? { startMs: cue.positionMs, endMs: cue.endMs } : null;
  seek(cue.positionMs);
}

// A click on each beat, to hear whether the grid sits on the music.
const metronome = ref(false);
/** The time of the last beat a click was scheduled for. */
let clickedUpTo = -1;

function scheduleClicks(ms: number) {
  const list = beats.value;
  // On the audio's own clock, so a click lands on its beat.
  const clicks = engine.context;
  if (!metronome.value || !list.length || !clicks) return;
  // After a jump, beats click again from the new place.
  if (ms < clickedUpTo - 200 || ms > clickedUpTo + 2000) clickedUpTo = ms - 1;
  for (let i = Math.max(0, nearestBeat(ms) - 1); i < list.length && list[i][0] <= ms + 120; i++) {
    const [at, bar] = list[i];
    if (at <= clickedUpTo || at < ms) continue;
    clickedUpTo = at;
    // `ms` is what's being heard, which was played a moment ago.
    const when = Math.max(clicks.currentTime, clicks.currentTime + (at - ms) / 1000 - engine.latency);
    const oscillator = clicks.createOscillator();
    const gain = clicks.createGain();
    oscillator.frequency.value = bar === 1 ? 1760 : 1175;
    gain.gain.setValueAtTime(0.25, when);
    gain.gain.exponentialRampToValueAtTime(0.001, when + 0.04);
    oscillator.connect(gain).connect(clicks.destination);
    oscillator.start(when);
    oscillator.stop(when + 0.05);
  }
}

// The waveforms are drawn when something they show has changed, and a frame
// is asked for only then or while playing: an idle player does no work.
let frame = 0;
let dirty = true;

function invalidate() {
  dirty = true;
  if (!frame) frame = requestAnimationFrame(tick);
}

function tick() {
  frame = 0;
  const running = engine.playing;
  if (running) {
    let ms = engine.positionMs();
    const loop = activeLoop.value;
    if (loop && ms >= loop.endMs) {
      engine.seek(loop.startMs);
      ms = loop.startMs;
    }
    position.value = ms;
    scheduleClicks(ms);
  }
  if (dirty) {
    dirty = false;
    drawDetail();
    drawOverview();
  }
  if (running && !frame) frame = requestAnimationFrame(tick);
}

function onKey(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null;
  if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  if (event.code === 'Space') toggle();
  else if (event.code === 'ArrowLeft') jumpBeats(event.shiftKey ? -4 : -1);
  else if (event.code === 'ArrowRight') jumpBeats(event.shiftKey ? 4 : 1);
  else return;
  event.preventDefault();
}

// --- Cues and loops ----------------------------------------------------------

/** Put new cues on the nearest beat. */
const quantize = ref(true);
/** New cues are loops this many beats long; 0 for plain cues. */
const loopBeats = ref(0);
const busy = ref(false);

/** Makes a change and reads the track again. False if it was refused. */
async function change(edit: CollectionEdit): Promise<boolean> {
  if (busy.value || !props.track) return false;
  busy.value = true;
  error.value = null;
  try {
    const outcome = await store.edit(edit);
    if (outcome === undefined) error.value = store.error;
    await loadPreparation(false);
    return outcome !== undefined;
  } finally {
    busy.value = false;
  }
}

/** Where a cue set now would go, and where it would end if it's a loop. */
function cuePoint(): { positionMs: number; endMs: number | null } {
  const at = nearestBeat(position.value);
  const list = beats.value;
  const start = quantize.value && at >= 0 ? list[at][0] : Math.round(position.value);
  if (!loopBeats.value) return { positionMs: start, endMs: null };
  let end: number;
  if (quantize.value && at >= 0 && at + loopBeats.value < list.length) {
    end = list[at + loopBeats.value][0];
  } else {
    const bpm = prep.value?.bpm ?? props.track?.bpm ?? 120;
    end = Math.round(start + (loopBeats.value * 60_000) / bpm);
  }
  return { positionMs: start, endMs: Math.min(end, Math.round(durationMs.value)) };
}

/** A pad with a cue jumps to it; an empty one takes a cue at the playhead. */
function pressPad(slot: number, event: MouseEvent) {
  if (!props.track) return;
  const cue = hotCues.value[slot];
  if (cue && event.shiftKey) return change({ kind: 'deleteCue', cueId: cue.id });
  if (cue) return goToCue(cue);
  change({ kind: 'addCue', trackId: props.track.id, slot, ...cuePoint() });
}

function addMemoryCue() {
  if (props.track) change({ kind: 'addCue', trackId: props.track.id, slot: null, ...cuePoint() });
}

function nameCue(cue: Cue, event: Event) {
  const comment = (event.target as HTMLInputElement).value.trim();
  if (comment !== cue.comment) change({ kind: 'nameCue', cueId: cue.id, comment });
}

// --- Beat grid ---------------------------------------------------------------

/** Change the grid only from the playhead on, for a track that changes tempo. */
const fromHere = ref(false);
/** Grids as they were before each change, newest last. */
const undoStack = ref<Beat[][]>([]);
const locked = computed(() => prep.value?.locked ?? false);
const canEditGrid = computed(() => !!beats.value.length && !locked.value && !busy.value);

async function grid(gridChange: GridChange) {
  if (!props.track) return;
  const before = beats.value;
  const done = await change({
    kind: 'grid',
    trackId: props.track.id,
    change: gridChange,
    fromMs: fromHere.value ? Math.round(position.value) : null,
  });
  if (done && before.length && JSON.stringify(before) !== JSON.stringify(beats.value)) {
    undoStack.value = [...undoStack.value.slice(-49), before];
  }
}

async function undo() {
  const last = undoStack.value[undoStack.value.length - 1];
  if (!last || !props.track) return;
  const done = await change({
    kind: 'grid',
    trackId: props.track.id,
    change: { kind: 'restore', beats: last },
    fromMs: null,
  });
  if (done) undoStack.value = undoStack.value.slice(0, -1);
}

/** Makes the beat nearest the playhead the first of its bar; nothing moves. */
function setDownbeat() {
  const at = nearestBeat(position.value);
  if (at >= 0) grid({ kind: 'downbeat', timeMs: beats.value[at][0] });
}

const bpmInput = ref('');
/** The tempo at the playhead, which is the grid's own unless it changes tempo. */
const bpmHere = computed(() => {
  const at = nearestBeat(position.value);
  return at >= 0 ? beats.value[at][2] / 100 : null;
});
watch(bpmHere, (bpm) => (bpmInput.value = bpm ? bpm.toFixed(2) : ''));

function commitBpm() {
  const bpm = Number(bpmInput.value);
  if (!bpm || bpm.toFixed(2) === bpmHere.value?.toFixed(2)) return;
  grid({ kind: 'tempo', bpm });
}

// Tap tempo: tap along, then use the tempo; a bar starts at the first tap.
const taps = ref<{ at: number; positionMs: number }[]>([]);
const tappedBpm = computed(() => {
  const list = taps.value;
  if (list.length < 4) return null;
  const bpm = (60_000 * (list.length - 1)) / (list[list.length - 1].at - list[0].at);
  return Math.round(bpm * 10) / 10;
});

function tap() {
  const now = performance.now();
  const list = taps.value;
  const stale = list.length && now - list[list.length - 1].at > 2000;
  taps.value = [...(stale ? [] : list), { at: now, positionMs: position.value }];
}

async function useTappedTempo() {
  const bpm = tappedBpm.value;
  if (!bpm) return;
  await grid({ kind: 'tap', bpm, anchorMs: Math.round(taps.value[0].positionMs) });
  taps.value = [];
}

function toggleLock() {
  if (props.track) change({ kind: 'lockAnalysis', trackId: props.track.id, locked: !locked.value });
}

// --- Drawing -----------------------------------------------------------------

const detail = ref<HTMLCanvasElement | null>(null);
const overview = ref<HTMLCanvasElement | null>(null);
/** Which tools show under the waveforms, as rekordbox's tabs do. */
const tools = ref<'cues' | 'grid'>('cues');
/** Seconds of audio across the detailed waveform. */
const ZOOMS = [2, 4, 8, 16, 32];
const zoom = ref(2);
/** The overview's waveform, drawn once per track, size and style. */
let overviewImage: { key: string; canvas: HTMLCanvasElement } | null = null;

function currentWave(): Wave | null {
  const p = palette.value;
  const data = p && waves.value.get(p.kind);
  return p && data ? { kind: p.kind, data, columns: columns.value } : null;
}

function drawDetail() {
  const canvas = detail.value;
  if (!canvas) return;
  const [width, height, c] = fit(canvas);
  c.fillStyle = '#000';
  c.fillRect(0, 0, width, height);
  const pxPerMs = width / (ZOOMS[zoom.value] * 1000);
  const startMs = position.value - width / 2 / pxPerMs;
  const endMs = startMs + width / pxPerMs;
  const x = (ms: number) => Math.round((ms - startMs) * pxPerMs);
  const mid = height / 2;
  /** Clear of the markers' heads and badges, top and bottom. */
  const reach = mid - 16;

  // Saved loops, behind the waveform.
  for (const cue of cues.value) {
    if (!cue.endMs || cue.endMs < startMs || cue.positionMs > endMs) continue;
    c.fillStyle = 'rgba(255,130,5,0.22)';
    c.fillRect(x(cue.positionMs), 0, (cue.endMs - cue.positionMs) * pxPerMs, height);
  }

  const wave = currentWave();
  if (wave) {
    const columnsPerPx = COLUMNS_PER_SEC / 1000 / pxPerMs;
    const firstColumn = (startMs / 1000) * COLUMNS_PER_SEC;
    for (let px = 0; px < width; px++) {
      const from = Math.floor(firstColumn + px * columnsPerPx);
      drawSlice(c, wave, px, from, Math.floor(firstColumn + (px + 1) * columnsPerPx), mid, reach, true);
    }
  }

  // The beat grid: a line through the whole waveform on every beat, grey,
  // and white with red heads where a bar starts, numbered.
  const list = beats.value;
  if (list.length) {
    const first = Math.max(0, nearestBeat(startMs) - 1);
    const spacing = list.length > 1 ? (list[Math.min(first + 1, list.length - 1)][0] - list[first][0]) * pxPerMs : 99;
    c.font = '10px ui-monospace, monospace';
    c.textBaseline = 'top';
    for (let i = first; i < list.length && list[i][0] <= endMs; i++) {
      const bar = list[i][1] === 1;
      // Too close to tell apart: only the bars are drawn.
      if (!bar && spacing < 5) continue;
      const at = x(list[i][0]);
      c.fillStyle = bar ? COLOR.downbeat : COLOR.beat;
      c.fillRect(at, 0, 1, height);
      c.fillStyle = bar ? COLOR.downbeatHead : COLOR.beatHead;
      head(c, at + 0.5, 0, bar ? 4 : 3, true);
      head(c, at + 0.5, height, bar ? 4 : 3, false);
      if (bar && spacing * 4 > 26) {
        c.fillStyle = 'rgba(255,255,255,0.75)';
        c.fillText(String(bars.value[i]), at + 6, height - 13);
      }
    }
  }

  // Cues: a hot cue is a lettered badge on a line; a memory cue a red head.
  c.font = '700 10px system-ui';
  c.textBaseline = 'middle';
  for (const cue of cues.value) {
    const at = x(cue.positionMs);
    if (at < -80 || at > width + 80) continue;
    const color = cueColor(cue);
    if (cue.slot === null) {
      c.fillStyle = color;
      c.fillRect(at, 0, 1, height);
      head(c, at + 0.5, 0, 6, true);
    } else {
      c.fillStyle = '#fff';
      c.fillRect(at, 0, 1, height);
      c.fillStyle = color;
      c.fillRect(at + 1, 1, 13, 13);
      c.fillStyle = '#000';
      c.textAlign = 'center';
      c.fillText(cueLabel(cue), at + 7.5, 8);
      c.textAlign = 'left';
    }
    if (cue.comment) {
      c.font = '10px system-ui';
      const left = at + (cue.slot === null ? 9 : 17);
      c.fillStyle = 'rgba(0,0,0,0.7)';
      c.fillRect(left - 2, 1, c.measureText(cue.comment).width + 4, 13);
      c.fillStyle = '#fff';
      c.fillText(cue.comment, left, 8);
      c.font = '700 10px system-ui';
    }
  }

  // The playhead, fixed in the middle: red while paused, white while playing.
  if (!props.track) return;
  c.fillStyle = playing.value ? COLOR.playheadPlaying : COLOR.playheadPaused;
  c.fillRect(Math.round(width / 2) - 1, 0, 2, height);
}

function drawOverview() {
  const canvas = overview.value;
  if (!canvas) return;
  const [width, height, c] = fit(canvas);
  c.fillStyle = '#000';
  c.fillRect(0, 0, width, height);
  const key = `${props.track?.id}/${palette.value?.id}/${columns.value}/${canvas.width}x${canvas.height}`;
  if (overviewImage?.key !== key) {
    const image = document.createElement('canvas');
    image.width = canvas.width;
    image.height = canvas.height;
    const ic = image.getContext('2d')!;
    ic.scale(canvas.width / width, canvas.height / height);
    const wave = currentWave();
    if (wave) {
      const perPx = wave.columns / width;
      for (let px = 0; px < width; px++) {
        drawSlice(ic, wave, px, Math.floor(px * perPx), Math.floor((px + 1) * perPx), height, height - 7, false);
      }
    }
    overviewImage = { key, canvas: image };
  }
  c.drawImage(overviewImage.canvas, 0, 0, width, height);
  if (!props.track) return;
  const total = durationMs.value || 1;
  const at = (ms: number) => Math.round((ms / total) * width);
  // What has played is dimmed.
  c.fillStyle = 'rgba(0,0,0,0.55)';
  c.fillRect(0, 0, at(position.value), height);
  for (const cue of cues.value) {
    c.fillStyle = cueColor(cue);
    if (cue.endMs) c.fillRect(at(cue.positionMs), 0, Math.max(1, at(cue.endMs) - at(cue.positionMs)), 2);
    head(c, at(cue.positionMs) + 0.5, 0, 3.5, true);
  }
  c.fillStyle = playing.value ? COLOR.playheadPlaying : COLOR.playheadPaused;
  c.fillRect(at(position.value) - 1, 0, 2, height);
}

// --- Pointer -----------------------------------------------------------------

/** Dragging the detailed waveform pulls the track under the playhead. */
let dragFrom: { x: number; ms: number } | null = null;
let overviewHeld = false;

function detailDown(event: PointerEvent) {
  dragFrom = { x: event.clientX, ms: position.value };
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function detailMove(event: PointerEvent) {
  if (!dragFrom || !detail.value) return;
  const msPerPx = (ZOOMS[zoom.value] * 1000) / detail.value.clientWidth;
  seek(dragFrom.ms - (event.clientX - dragFrom.x) * msPerPx);
}

function detailWheel(event: WheelEvent) {
  if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
    if (!detail.value) return;
    seek(position.value + event.deltaX * ((ZOOMS[zoom.value] * 1000) / detail.value.clientWidth));
  } else if (event.deltaY) {
    setZoom(zoom.value + (event.deltaY > 0 ? 1 : -1));
  }
}

function setZoom(level: number) {
  zoom.value = Math.max(0, Math.min(ZOOMS.length - 1, level));
}

function overviewSeek(event: PointerEvent) {
  const canvas = overview.value;
  if (!canvas) return;
  const rect = canvas.getBoundingClientRect();
  activeLoop.value = null;
  seek(((event.clientX - rect.left) / rect.width) * durationMs.value);
}

function overviewDown(event: PointerEvent) {
  overviewHeld = true;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  overviewSeek(event);
}

function overviewMove(event: PointerEvent) {
  if (overviewHeld) overviewSeek(event);
}

function pointerUp() {
  overviewHeld = false;
  dragFrom = null;
}

function formatTime(ms: number) {
  const total = Math.max(0, ms) / 1000;
  const minutes = Math.floor(total / 60);
  return `${minutes}:${(total - minutes * 60).toFixed(1).padStart(4, '0')}`;
}

/** The time readout: a computed, so the player re-renders when the text changes, not every frame. */
const timeLabel = computed(() => `${formatTime(position.value)} / ${formatTime(durationMs.value)}`);

/** The playhead's bar and beat, as rekordbox counts them. */
const barAndBeat = computed(() => {
  const at = nearestBeat(position.value);
  if (at < 0) return null;
  const list = beats.value;
  const i = list[at][0] > position.value + 5 ? at - 1 : at;
  return i >= 0 ? `${bars.value[i]}.${list[i][1]}` : null;
});

// Last, once everything loading touches exists.
watch(
  () => props.track?.id,
  () => {
    loadTrack();
  },
  { immediate: true },
);
// An analysis made elsewhere (rekordbox, then Refresh) shows as a new BPM on the row.
watch([() => props.track?.bpm, () => props.track?.analysed], () => loadPreparation());
// Another of the analysis's grids tried from the track's details.
watch(
  () => [props.track?.id, props.track?.grid?.using] as const,
  ([id, using], [wasId, wasUsing]) => {
    if (id && id === wasId && using !== wasUsing && !busy.value) loadPreparation(false);
  },
);
// One made here is done when the track leaves the queue's running list.
const analysing = computed(() => !!props.track && (store.analysis?.running.includes(props.track.id) ?? false));
watch(analysing, (now, was) => {
  if (was && !now) loadPreparation();
});
// And Refresh reads cues and grids changed in rekordbox.
watch(
  () => store.loading,
  (now, was) => {
    if (was && !now) loadPreparation();
  },
);

// Everything the waveforms show. Sync, so a change made during a frame is
// drawn by that frame.
watch(
  [position, zoom, palette, waves, prep, playing, durationMs, () => props.track?.id],
  invalidate,
  { flush: 'sync' },
);
// The canvases' own size, and the screen's pixel density.
const resized = new ResizeObserver(invalidate);

onMounted(() => {
  if (detail.value) resized.observe(detail.value);
  if (overview.value) resized.observe(overview.value);
  invalidate();
  window.addEventListener('resize', invalidate);
  window.addEventListener('keydown', onKey);
});
onBeforeUnmount(() => {
  cancelAnimationFrame(frame);
  resized.disconnect();
  window.removeEventListener('resize', invalidate);
  window.removeEventListener('keydown', onKey);
  engine.dispose();
});

const buttonClass =
  'rounded bg-surface-2 px-2 py-1 text-[11px] transition-colors hover:bg-surface-3 disabled:opacity-40';
const onClass = 'rounded bg-accent px-2 py-1 text-[11px] text-white transition-colors disabled:opacity-40';
const tabClass = 'px-2 py-0.5 text-[11px] disabled:opacity-30';
</script>

<template>
  <section class="flex shrink-0 flex-col gap-1.5 border-b border-border px-4 pb-2 text-xs">
    <div class="flex h-8 items-center gap-3">
      <button
        class="flex size-7 shrink-0 items-center justify-center rounded-full bg-surface-3 text-sm hover:bg-accent disabled:opacity-40"
        :disabled="!ready"
        :title="playing ? 'Pause (Space)' : 'Play (Space)'"
        @click="toggle"
      >
        {{ playing ? '❚❚' : '▶' }}
      </button>
      <div class="min-w-0 flex-1">
        <template v-if="track">
          <p class="truncate leading-tight">
            {{ track.title }}
            <span class="text-text-secondary">· {{ track.artist }}</span>
          </p>
          <p class="font-mono text-[11px] leading-tight text-text-muted">
            <template v-if="loading">Decoding…</template>
            <template v-else>{{ timeLabel }}</template>
            <span v-if="barAndBeat" class="ml-2" title="Bar and beat at the playhead">{{ barAndBeat }}</span>
            <span v-if="bpmHere" class="ml-2">{{ bpmHere.toFixed(2) }} BPM</span>
            <span v-if="track.key" class="ml-2">{{ formatKey(track.key) }}</span>
            <span v-if="activeLoop" class="ml-2 text-status-busy">
              Looping
              <button class="underline" @click="activeLoop = null">exit</button>
            </span>
          </p>
        </template>
        <p v-else class="text-text-muted">No track loaded</p>
      </div>
      <template v-if="track">
        <button :class="buttonClass" title="Back a beat (←, Shift for a bar)" @click="jumpBeats(-1)">◀ Beat</button>
        <button :class="buttonClass" title="Forward a beat (→, Shift for a bar)" @click="jumpBeats(1)">Beat ▶</button>
      </template>
      <div class="flex overflow-hidden rounded border border-border" title="Waveform colours, as in rekordbox">
        <button
          v-for="p in PALETTES"
          :key="p.id"
          :class="[tabClass, (palette?.id ?? chosenPalette) === p.id ? 'bg-surface-3 text-text-primary' : 'text-text-muted hover:bg-surface-2']"
          :disabled="!!track && !!waves.size && !waves.has(p.kind)"
          @click="chosenPalette = p.id"
        >
          {{ p.label }}
        </button>
      </div>
      <div class="flex items-center gap-1">
        <button :class="buttonClass" title="Zoom out" :disabled="zoom === ZOOMS.length - 1" @click="setZoom(zoom + 1)">−</button>
        <button :class="buttonClass" title="Zoom in" :disabled="zoom === 0" @click="setZoom(zoom - 1)">+</button>
      </div>
      <button
        v-if="track"
        class="text-text-muted hover:text-text-primary"
        title="Unload the track"
        @click="$emit('close')"
      >✕</button>
    </div>

    <p v-if="error" class="error-box">{{ error }}</p>

    <!-- Fixed-height boxes: the canvases fill them and never size the layout. -->
    <div class="relative h-24 overflow-hidden rounded-sm bg-black">
      <canvas
        ref="detail"
        class="absolute inset-0 size-full touch-none"
        :class="track ? 'cursor-grab active:cursor-grabbing' : ''"
        :title="track ? 'Drag to move through the track. Scroll to zoom.' : undefined"
        @pointerdown="detailDown"
        @pointermove="detailMove"
        @pointerup="pointerUp"
        @wheel.prevent="detailWheel"
      />
      <p
        v-if="!track || (!waves.size && !loading)"
        class="pointer-events-none absolute inset-0 flex items-center justify-center text-text-muted"
      >
        <template v-if="!track">Double-click a track to load it</template>
        <template v-else>No waveform yet: analyse the track to get one, and a beat grid.</template>
      </p>
    </div>
    <div class="relative h-7 overflow-hidden rounded-sm bg-black">
      <canvas
        ref="overview"
        class="absolute inset-0 size-full"
        :class="track ? 'cursor-pointer' : ''"
        @pointerdown="overviewDown"
        @pointermove="overviewMove"
        @pointerup="pointerUp"
      />
    </div>

    <div class="flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1.5" :class="track ? '' : 'pointer-events-none opacity-40'">
      <div class="flex shrink-0 overflow-hidden rounded border border-border">
        <button
          :class="[tabClass, tools === 'cues' ? 'bg-surface-3 text-text-primary' : 'text-text-muted hover:bg-surface-2']"
          @click="tools = 'cues'"
        >
          Cues
        </button>
        <button
          :class="[tabClass, tools === 'grid' ? 'bg-surface-3 text-text-primary' : 'text-text-muted hover:bg-surface-2']"
          title="Beat grid and tempo"
          @click="tools = 'grid'"
        >
          Grid
        </button>
      </div>

      <template v-if="tools === 'cues'">
        <div class="flex items-center gap-1">
          <button
            v-for="(letter, slot) in PADS"
            :key="letter"
            class="size-6 rounded text-[11px] font-semibold transition-colors disabled:opacity-40"
            :class="hotCues[slot] ? 'text-black' : 'bg-surface-2 text-text-muted hover:bg-surface-3'"
            :style="hotCues[slot] ? { backgroundColor: cueColor(hotCues[slot]!) } : {}"
            :disabled="busy"
            :title="hotCues[slot]
              ? `Hot cue ${letter} at ${formatTime(hotCues[slot]!.positionMs)}. Click to go there, Shift-click to delete.`
              : `Set hot cue ${letter} at the playhead`"
            @click="pressPad(slot, $event)"
          >
            {{ letter }}
          </button>
        </div>
        <button :class="buttonClass" :disabled="busy" title="Set a memory cue at the playhead" @click="addMemoryCue">
          + Memory cue
        </button>
        <label class="flex items-center gap-1 text-[11px] text-text-secondary" title="New cues are loops of this length">
          Set as
          <select v-model.number="loopBeats" class="rounded border border-border bg-surface-1 px-1 py-0.5 text-[11px] outline-none">
            <option :value="0">cue</option>
            <option v-for="n in LOOP_LENGTHS" :key="n" :value="n">{{ n }}-beat loop</option>
          </select>
        </label>
        <label class="flex items-center gap-1 text-[11px] text-text-secondary" title="Put new cues on the nearest beat">
          <input v-model="quantize" type="checkbox" class="accent-accent" />
          Quantize
        </label>
      </template>

      <template v-else>
        <button
          :class="buttonClass"
          class="font-medium"
          :disabled="!canEditGrid"
          title="Make the beat nearest the playhead the first beat of its bar. The grid doesn't move."
          @click="setDownbeat"
        >
          Set downbeat
        </button>
        <button
          :class="buttonClass"
          :disabled="!canEditGrid"
          title="Slide the grid so its nearest beat is exactly at the playhead"
          @click="grid({ kind: 'align', timeMs: Math.round(position) })"
        >
          Grid to playhead
        </button>
        <div class="flex items-center gap-1" title="Slide the grid earlier or later by 1 ms (Shift: 10 ms)">
          <span class="text-[11px] text-text-secondary">Move</span>
          <button :class="buttonClass" :disabled="!canEditGrid" @click="grid({ kind: 'nudge', ms: $event.shiftKey ? -10 : -1 })">◀</button>
          <button :class="buttonClass" :disabled="!canEditGrid" @click="grid({ kind: 'nudge', ms: $event.shiftKey ? 10 : 1 })">▶</button>
        </div>
        <div
          class="flex items-center gap-1"
          title="Narrow or widen the gaps between beats: the beat nearest the playhead moves 1 ms (Shift: 5 ms) while the first beat stays. Go far into the track for a fine adjustment."
        >
          <span class="text-[11px] text-text-secondary">Spacing</span>
          <button :class="buttonClass" :disabled="!canEditGrid" @click="grid({ kind: 'stretch', byMs: $event.shiftKey ? -5 : -1, timeMs: Math.round(position) })">▶◀</button>
          <button :class="buttonClass" :disabled="!canEditGrid" @click="grid({ kind: 'stretch', byMs: $event.shiftKey ? 5 : 1, timeMs: Math.round(position) })">◀▶</button>
        </div>
        <div class="flex items-center gap-1">
          <span class="text-[11px] text-text-secondary">BPM</span>
          <input
            v-model="bpmInput"
            :disabled="!canEditGrid"
            class="w-16 rounded border border-border bg-surface-1 px-1.5 py-0.5 text-right font-mono text-[11px] outline-none focus:border-border-active disabled:opacity-40"
            title="The grid's tempo. Changing it respaces the beats from the first one."
            spellcheck="false"
            @change="commitBpm"
            @keydown.enter="($event.target as HTMLInputElement).blur()"
          />
          <button :class="buttonClass" :disabled="!canEditGrid" title="Halve the tempo: every other beat goes" @click="grid({ kind: 'halve' })">÷2</button>
          <button :class="buttonClass" :disabled="!canEditGrid" title="Double the tempo: a beat is added between each pair" @click="grid({ kind: 'double' })">×2</button>
        </div>
        <div class="flex items-center gap-1">
          <button
            :class="buttonClass"
            :disabled="!canEditGrid"
            title="Tap along with the music, four times or more, to measure its tempo"
            @click="tap"
          >
            Tap{{ taps.length ? ` ${taps.length}` : '' }}
          </button>
          <button
            v-if="tappedBpm"
            :class="onClass"
            :disabled="!canEditGrid"
            title="Respace the grid at the tapped tempo, with a bar starting where you first tapped"
            @click="useTappedTempo"
          >
            Use {{ tappedBpm.toFixed(1) }}
          </button>
        </div>
        <label
          class="flex items-center gap-1 text-[11px] text-text-secondary"
          title="Change the grid only from the playhead on, leaving the beats before it: for a track that changes tempo"
        >
          <input v-model="fromHere" type="checkbox" class="accent-accent" />
          From playhead on
        </label>
        <div class="ml-auto flex items-center gap-1">
          <button
            :class="metronome ? onClass : buttonClass"
            :disabled="!beats.length"
            title="Click on every beat while playing, to hear whether the grid sits on the music"
            @click="metronome = !metronome"
          >
            Metronome
          </button>
          <button :class="buttonClass" :disabled="!undoStack.length || busy || locked" title="Undo the last grid change" @click="undo">
            Undo
          </button>
          <button
            :class="locked ? onClass : buttonClass"
            :disabled="!beats.length || busy"
            :title="locked
              ? 'The grid is locked, here and in rekordbox: it can\'t be changed or analysed over. Click to unlock.'
              : 'Lock the grid, here and in rekordbox, so it can\'t be changed or analysed over'"
            @click="toggleLock"
          >
            {{ locked ? 'Locked' : 'Lock' }}
          </button>
        </div>
      </template>
    </div>

    <!-- Every cue, to name or delete: one line, scrolling sideways. -->
    <ul v-if="tools === 'cues' && cues.length" class="flex gap-x-3 overflow-x-auto whitespace-nowrap">
      <li v-for="cue in cues" :key="cue.id" class="flex shrink-0 items-center gap-1">
        <button
          class="rounded px-1 text-[10px] font-semibold text-black"
          :style="{ backgroundColor: cueColor(cue) }"
          :title="cue.endMs ? 'Play this loop' : 'Go to this cue'"
          @click="goToCue(cue)"
        >
          {{ cueLabel(cue) }}{{ cue.endMs ? ' ↻' : '' }}
        </button>
        <span class="font-mono text-[10px] text-text-muted">{{ formatTime(cue.positionMs) }}</span>
        <input
          :value="cue.comment"
          placeholder="name"
          spellcheck="false"
          class="w-20 rounded border border-transparent bg-transparent px-1 text-[11px] outline-none hover:border-border focus:border-border-active"
          @change="nameCue(cue, $event)"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <button
          class="text-text-muted hover:text-status-error"
          title="Delete this cue"
          :disabled="busy"
          @click="change({ kind: 'deleteCue', cueId: cue.id })"
        >✕</button>
      </li>
    </ul>
  </section>
</template>
