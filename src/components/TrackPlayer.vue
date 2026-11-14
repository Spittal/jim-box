<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import LibraryCover from '@/components/LibraryCover.vue';
import VolumeKnob from '@/components/VolumeKnob.vue';
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
import { editedGrid, nearestBeatIndex, sameGrid } from '@/composables/useGridEdit';
import { useKeyNotation } from '@/composables/useKeyNotation';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useUiStore } from '@/stores/ui';
import { commandKey, shortcut } from '@/platform';
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
const browse = useBrowseStore();
const ui = useUiStore();
const { formatKey } = useKeyNotation();

/** rekordbox's detailed waveforms have 150 columns a second. */
const COLUMNS_PER_SEC = 150;
const PADS = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'];
/** The lengths a loop can be, in beats: one and two beats, then one bar to thirty-two. */
const LOOP_SIZES = [1, 2, 4, 8, 16, 32, 64, 128];
/** The tempo can be played this much slower or faster than the track's own. */
const SPEED_RANGE = [0.5, 2] as const;

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
    // Changes still being written are ahead of what was read: the read after
    // the last of them is the one to show.
    if (!pending.value) prep.value = nextPrep;
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
  speed.value = 1;
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
// What's done to the list knows whether its track is being heard (see useTrackActions.ts).
watch(playing, (now) => (browse.playing = now), { immediate: true });
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
const nearestBeat = (ms: number) => nearestBeatIndex(beats.value, ms);

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
// The audio thread goes round it, to the sample. Sync, so it's told before a seek that follows.
watch(activeLoop, (loop) => engine.setLoop(loop), { flush: 'sync' });

/** Which of `LOOP_SIZES` a loop set now is. */
const loopSize = ref(LOOP_SIZES.indexOf(32));
const loopLabel = computed(() => {
  const count = LOOP_SIZES[loopSize.value];
  return count < 4 ? `${count} beat` : `${count / 4} bar`;
});

/** Where a loop of so many beats from `startMs` ends: on the grid's own beat where it starts on one. */
function loopEnd(startMs: number, count: number): number {
  const list = beats.value;
  const at = nearestBeat(startMs);
  let end: number;
  if (at >= 0 && Math.abs(list[at][0] - startMs) < 2 && at + count < list.length) end = list[at + count][0];
  else {
    const bpm = (at >= 0 ? list[at][2] / 100 : null) ?? prep.value?.bpm ?? props.track?.bpm ?? 120;
    end = startMs + (count * 60_000) / bpm;
  }
  return Math.min(Math.round(end), Math.round(durationMs.value));
}

/** Starts a loop of the chosen length at the playhead, or leaves the one playing. */
function toggleLoop() {
  if (activeLoop.value) return void (activeLoop.value = null);
  if (!props.track) return;
  const at = nearestBeat(position.value);
  const startMs = quantize.value && at >= 0 ? beats.value[at][0] : Math.round(position.value);
  const endMs = loopEnd(startMs, LOOP_SIZES[loopSize.value]);
  if (endMs > startMs) activeLoop.value = { startMs, endMs };
}

/** Halves or doubles the loop's length; one playing changes where it ends. */
function stepLoop(by: number) {
  const next = Math.max(0, Math.min(LOOP_SIZES.length - 1, loopSize.value + by));
  if (next === loopSize.value) return;
  loopSize.value = next;
  const loop = activeLoop.value;
  if (!loop) return;
  const endMs = loopEnd(loop.startMs, LOOP_SIZES[next]);
  if (endMs > loop.startMs) activeLoop.value = { startMs: loop.startMs, endMs };
}

// --- Volume and tempo ----------------------------------------------------------

/** The master volume knob, 0 to 1, kept between runs. */
const savedVolume = Number(localStorage.getItem('player.volume') ?? 1);
const volume = ref(savedVolume >= 0 && savedVolume <= 1 ? savedVolume : 1);
watch(
  volume,
  (level) => {
    // Squared: evenly louder to the ear as the knob turns.
    engine.setVolume(level * level);
    localStorage.setItem('player.volume', String(level));
  },
  { immediate: true },
);

/**
 * How fast the track is played, 1 being as recorded. The samples are stepped
 * through faster or slower, so the pitch goes with the tempo as on a turntable:
 * nothing is stretched. It's the playing only; the track and its grid are as they were.
 */
const speed = ref(1);
watch(speed, (now) => engine.setSpeed(now), { flush: 'sync' });

function goToCue(cue: Cue) {
  activeLoop.value = cue.endMs ? { startMs: cue.positionMs, endMs: cue.endMs } : null;
  seek(cue.positionMs);
}

// A click on each beat, to hear whether the grid sits on the music.
const metronome = ref(false);
/** The clicks' own volume knob, 0 to 1, kept between runs; the master volume is on top of it. */
const savedClickVolume = Number(localStorage.getItem('player.metronomeVolume') ?? 0.35);
const clickVolume = ref(savedClickVolume >= 0 && savedClickVolume <= 1 ? savedClickVolume : 0.35);
watch(clickVolume, (level) => localStorage.setItem('player.metronomeVolume', String(level)));
/** A click's gain with the knob at full: well over the music, to be heard through a loud track. */
const CLICK_GAIN = 2;
/** The time of the last beat a click was scheduled for. */
let clickedUpTo = -1;

function scheduleClicks(ms: number) {
  const list = beats.value;
  // On the audio's own clock, so a click lands on its beat.
  const clicks = engine.context;
  if (!metronome.value || !list.length || !clicks) return;
  // Squared, as the master volume is. Silent clicks aren't made at all.
  const level = CLICK_GAIN * clickVolume.value * clickVolume.value;
  if (level < 0.001) return;
  // After a jump, beats click again from the new place.
  if (ms < clickedUpTo - 200 || ms > clickedUpTo + 2000) clickedUpTo = ms - 1;
  for (let i = Math.max(0, nearestBeat(ms) - 1); i < list.length && list[i][0] <= ms + 120; i++) {
    const [at, bar] = list[i];
    if (at <= clickedUpTo || at < ms) continue;
    clickedUpTo = at;
    // `ms` is what's being heard, which was played a moment ago.
    const when = Math.max(clicks.currentTime, clicks.currentTime + (at - ms) / 1000 / speed.value - engine.latency);
    const oscillator = clicks.createOscillator();
    const gain = clicks.createGain();
    oscillator.frequency.value = bar === 1 ? 1760 : 1175;
    gain.gain.setValueAtTime(level, when);
    gain.gain.exponentialRampToValueAtTime(0.001, when + 0.04);
    oscillator.connect(gain).connect(engine.output ?? clicks.destination);
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
    const ms = engine.positionMs();
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

/** How far ⌘, and ⌘. go, and ⇧, and ⇧.: a bar, and a phrase, to the next part of the track. */
const SMALL_JUMP_BARS = 1;
const BIG_JUMP_BARS = 8;

/** Whether a key pressed there is typed: a box of text, not a tick box or a button. */
function typing(target: HTMLElement | null): boolean {
  if (!target) return false;
  if (target.tagName === 'TEXTAREA' || target.isContentEditable) return true;
  return (
    target instanceof HTMLInputElement &&
    !['checkbox', 'radio', 'button', 'submit', 'range', 'color', 'file'].includes(target.type)
  );
}

/** Space went to the player, so letting go of it mustn't press what has the keyboard. */
let spaceTaken = false;

function onKey(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null;
  if (event.defaultPrevented || event.altKey || ui.settings) return;
  // Space plays and pauses wherever the keyboard is, unless it's typing a space:
  // on a button, a menu or a tick box it doesn't press, open or tick it.
  if (event.code === 'Space' && !event.metaKey && !event.ctrlKey && !typing(target)) {
    if (!event.repeat) toggle();
    spaceTaken = true;
    event.preventDefault();
    return;
  }
  // Text fields keep their own keys: digits are typed, ⌘← goes to the start of the line.
  if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
  const command = commandKey(event);
  if (!command && (event.metaKey || event.ctrlKey)) return;
  // With ⌘, the digits are the picked track's stars (see CollectionTrackDetail.vue), not pads.
  const pad = !command && /^Digit[1-8]$/.test(event.code) ? Number(event.code.slice(5)) - 1 : -1;

  // Going back while it plays allows a beat for the one being played; paused, it's the plain count.
  const lead = engine.playing ? 1 : 0;

  if (command) {
    if (!props.track || event.shiftKey) return;
    if (event.code === 'Comma') jumpBeats((-SMALL_JUMP_BARS * 4) + lead);
    else if (event.code === 'Period') jumpBeats(SMALL_JUMP_BARS * 4);
    else if (event.code === 'KeyM') {
      if (!event.repeat) setCue(null);
    } else if (event.code === 'KeyB') {
      if (canEditGrid.value && !event.repeat) setDownbeat();
    } else return;
  } else if (event.code === 'Comma') jumpBeats(event.shiftKey ? (-BIG_JUMP_BARS * 4) + lead : -1 - lead);
  else if (event.code === 'Period') jumpBeats(event.shiftKey ? BIG_JUMP_BARS * 4 : 1);
  else if (!props.track) return;
  else if (pad >= 0) {
    // As pressing the pad: to its cue, or a cue set on it where it has none. With Shift, as Shift-clicking it: the cue goes.
    const cue = hotCues.value[pad];
    if (event.shiftKey) {
      if (cue && !event.repeat) deleteCue(cue);
    } else if (cue) goToCue(cue);
    else if (!event.repeat) setCue(pad);
  } else if (event.code === 'KeyM' && !event.shiftKey) {
    if (!event.repeat) setCue(null);
  } else if (event.code === 'KeyQ' && !event.shiftKey) {
    if (!event.repeat) quantize.value = !quantize.value;
  } else if (event.code === 'KeyG' && !event.shiftKey) {
    if (!event.repeat) gridTools.value = !gridTools.value;
  } else return;
  event.preventDefault();
}

/** A button is pressed as Space is let go, unless that is stopped here. */
function onKeyUp(event: KeyboardEvent) {
  if (event.code !== 'Space' || !spaceTaken) return;
  spaceTaken = false;
  event.preventDefault();
}

// --- Cues and loops ----------------------------------------------------------

/** Put new cues on the nearest beat. */
const quantize = ref(true);

// A change shows at once, as it will be once written, and is written behind
// the scenes in the order it was made: writing one reads rekordbox's whole
// library again, which is no time to make a button wait.

/** A change shown, and waiting to be written. */
interface Write {
  edit: CollectionEdit;
  trackId: string;
  /** `failures` when it was made. */
  batch: number;
  /** The undo stack before it, to go back to if it's refused. */
  undo: Beat[][];
  /** The id a new cue is shown under until rekordbox's is known. */
  cueId?: string;
  /** Run once written, before the next change is. */
  settle?: () => Promise<void>;
  done: ((written: boolean) => void)[];
}

const queue: Write[] = [];
let writing = false;
/** Changes shown that are still to be written. */
const pending = ref(0);
/** Bumped when a change is refused: those made on top of it aren't written. */
let failures = 0;
/** The id rekordbox gave each cue set here, by the id it was shown under. */
const cueIds = new Map<string, string>();
/** The other way round: a cue keeps its place in the list when its id arrives. */
const cueKeys = new Map<string, string>();
let cuesMade = 0;

/** Two nudges of the whole grid in a row are one, twice as far. */
function joined(first: CollectionEdit, then: CollectionEdit): CollectionEdit | null {
  if (first.kind !== 'grid' || then.kind !== 'grid' || first.trackId !== then.trackId) return null;
  if (first.fromMs !== null || then.fromMs !== null) return null;
  if (first.change.kind !== 'nudge' || then.change.kind !== 'nudge') return null;
  return { ...first, change: { kind: 'nudge', ms: first.change.ms + then.change.ms } };
}

/**
 * Makes a change: `show` puts it on screen now, and it's written when the
 * ones before it have been. Resolves to false if it was refused, after
 * which what is really there is read and shown.
 */
function change(
  edit: CollectionEdit,
  { show, settle, cueId }: { show?: () => void; settle?: () => Promise<void>; cueId?: string } = {},
): Promise<boolean> {
  if (!props.track) return Promise.resolve(false);
  const trackId = props.track.id;
  error.value = null;
  const undo = undoStack.value;
  show?.();
  return new Promise((done) => {
    const last = queue[queue.length - 1];
    const both = last && !last.settle && !settle && last.batch === failures ? joined(last.edit, edit) : null;
    if (last && both) {
      last.edit = both;
      last.done.push(done);
      return;
    }
    queue.push({ edit, trackId, batch: failures, undo, cueId, settle, done: [done] });
    pending.value++;
    void write();
  });
}

/** Waiting for the changes shown to have been written. */
const idlers: (() => void)[] = [];
// An undo is of the last change made, which has to be written first.
const stopSettling = store.onSettle(() =>
  writing || queue.length ? new Promise<void>((done) => idlers.push(done)) : Promise.resolve(),
);
onBeforeUnmount(stopSettling);

// An undo or redo of this track's cues, grid or lock: read them again. The
// grids kept here to go back to no longer lead to the one it has now.
watch(
  () => store.undone,
  (last) => {
    if (!props.track || !last?.trackIds.includes(props.track.id)) return;
    undoStack.value = [];
    void loadPreparation(false);
  },
);

async function write() {
  if (writing) return;
  writing = true;
  while (queue.length) {
    const next = queue.shift()!;
    let written = false;
    try {
      if (next.batch === failures) {
        // A cue set a moment ago is known to rekordbox by another id.
        const edit =
          'cueId' in next.edit && cueIds.has(next.edit.cueId)
            ? { ...next.edit, cueId: cueIds.get(next.edit.cueId)! }
            : next.edit;
        const outcome = await store.edit(edit, { quiet: true });
        written = outcome !== undefined;
        if (!written) {
          failures++;
          if (props.track?.id === next.trackId) {
            error.value = store.error;
            undoStack.value = next.undo;
          }
        } else {
          if (next.cueId && outcome) {
            cueIds.set(next.cueId, outcome);
            cueKeys.set(outcome, next.cueId);
          }
          await next.settle?.();
        }
      }
    } catch (e) {
      error.value = String(e);
    }
    pending.value--;
    for (const done of next.done) done(written);
  }
  writing = false;
  for (const done of idlers.splice(0)) done();
  // Once, for however many changes: the lists show cues and tempos too.
  store.touch();
  await loadPreparation(false);
}

/** Where a cue set now would go: the playhead, or the loop being played, which it then keeps. */
function cuePoint(): { positionMs: number; endMs: number | null } {
  const loop = activeLoop.value;
  if (loop) return { positionMs: Math.round(loop.startMs), endMs: Math.round(loop.endMs) };
  const at = nearestBeat(position.value);
  const start = quantize.value && at >= 0 ? beats.value[at][0] : Math.round(position.value);
  return { positionMs: start, endMs: null };
}

/** Sets a cue at the playhead: on a pad, in place of what's on it, or a memory cue. */
function setCue(slot: number | null) {
  if (!props.track) return;
  const at = cuePoint();
  if (slot === null && cues.value.some((c) => c.slot === null && c.positionMs === at.positionMs && c.endMs === at.endMs)) {
    error.value = "There's a memory cue there already";
    return;
  }
  const id = `new-${++cuesMade}`;
  change(
    { kind: 'addCue', trackId: props.track.id, slot, ...at },
    {
      cueId: id,
      show: () => {
        const current = prep.value;
        if (!current) return;
        const kept = current.cues.filter((c) => slot === null || c.slot !== slot);
        // In the colour rekordbox will draw it in, so it doesn't change when written.
        const color = slot === null ? null : at.endMs ? current.loopColor : (current.padColors[slot % 8] ?? null);
        const cue: Cue = { id, slot, ...at, comment: '', color };
        prep.value = {
          ...current,
          cues: [...kept, cue].sort((a, b) => a.positionMs - b.positionMs || (a.slot ?? -1) - (b.slot ?? -1)),
        };
      },
    },
  );
}

function deleteCue(cue: Cue) {
  change(
    { kind: 'deleteCue', cueId: cue.id },
    {
      show: () => {
        if (prep.value) prep.value = { ...prep.value, cues: prep.value.cues.filter((c) => c.id !== cue.id) };
      },
    },
  );
}

/** A pad with a cue jumps to it; an empty one takes a cue at the playhead. */
function pressPad(slot: number, event: MouseEvent) {
  if (!props.track) return;
  const cue = hotCues.value[slot];
  if (cue && event.shiftKey) return deleteCue(cue);
  if (cue) return goToCue(cue);
  setCue(slot);
}

/**
 * What is being typed in a box that shows a value. Every redraw puts the
 * value back in such a box, and playing redraws many times a second: so
 * while it's typed in, the box shows this instead.
 */
const typed = ref<{ box: string; text: string } | null>(null);

function shown(box: string, value: string) {
  return typed.value?.box === box ? typed.value.text : value;
}

function type(box: string, event: Event) {
  typed.value = { box, text: (event.target as HTMLInputElement).value };
}

function nameCue(cue: Cue, event: Event) {
  const comment = (event.target as HTMLInputElement).value.trim();
  if (comment === cue.comment) return;
  change(
    { kind: 'nameCue', cueId: cue.id, comment },
    {
      show: () => {
        if (!prep.value) return;
        prep.value = { ...prep.value, cues: prep.value.cues.map((c) => (c.id === cue.id ? { ...c, comment } : c)) };
      },
    },
  );
}

// --- Beat grid ---------------------------------------------------------------

/** Whether the beat grid's tools are showing under the cues', kept between runs. */
const gridTools = ref(localStorage.getItem('player.gridTools') === 'true');
watch(gridTools, (shown) => localStorage.setItem('player.gridTools', String(shown)));

/** Change the grid only from the playhead on, for a track that changes tempo. */
const fromHere = ref(false);
/** Grids as they were before each change, newest last. */
const undoStack = ref<Beat[][]>([]);
const locked = computed(() => prep.value?.locked ?? false);
/** A grid change that can't be shown until it's written is being made. */
const waiting = ref(false);
const canEditGrid = computed(() => !!beats.value.length && !locked.value && !waiting.value);

function showGrid(next: Beat[]) {
  if (prep.value) prep.value = { ...prep.value, beats: next, bpm: next.length ? next[0][2] / 100 : prep.value.bpm };
}

function grid(gridChange: GridChange): Promise<boolean> {
  const current = prep.value;
  if (!props.track || !current) return Promise.resolve(false);
  const trackId = props.track.id;
  const fromMs = fromHere.value ? Math.round(position.value) : null;
  const edit: CollectionEdit = { kind: 'grid', trackId, change: gridChange, fromMs };
  const remember = (before: Beat[]) => (undoStack.value = [...undoStack.value.slice(-49), before]);

  const next = editedGrid(current.beats, gridChange, fromMs, current.durationMs);
  if (next) {
    if (sameGrid(next, current.beats)) return Promise.resolve(true);
    return change(edit, {
      show: () => {
        remember(current.beats);
        showGrid(next);
      },
    });
  }
  // Not one to work out here: the grid waits to be told what it came to.
  waiting.value = true;
  return change(edit, {
    settle: async () => {
      const read = await store.preparation(trackId);
      if (props.track?.id !== trackId || !prep.value) return;
      const before = beats.value;
      showGrid(read.beats);
      if (before.length && !sameGrid(before, read.beats)) remember(before);
    },
  }).finally(() => (waiting.value = false));
}

function undo() {
  const stack = undoStack.value;
  const last = stack[stack.length - 1];
  if (!last || !props.track) return;
  change(
    { kind: 'grid', trackId: props.track.id, change: { kind: 'restore', beats: last }, fromMs: null },
    {
      show: () => {
        undoStack.value = stack.slice(0, -1);
        showGrid(last);
      },
    },
  );
}

/**
 * Makes the beat nearest the playhead the first of its bar. With quantize on
 * nothing moves; with it off the grid slides to put that beat at the playhead.
 */
function setDownbeat() {
  const at = nearestBeat(position.value);
  if (at >= 0) grid({ kind: 'downbeat', timeMs: quantize.value ? beats.value[at][0] : Math.round(position.value) });
}

const bpmInput = ref('');
/** The tempo at the playhead, which is the grid's own unless it changes tempo. */
const bpmHere = computed(() => {
  const at = nearestBeat(position.value);
  return at >= 0 ? beats.value[at][2] / 100 : null;
});
watch(bpmHere, (bpm) => (bpmInput.value = bpm ? bpm.toFixed(2) : ''));

/** The tempo being heard: the grid's, at the speed it's played. */
const playBpm = computed(() => (bpmHere.value ? bpmHere.value * speed.value : null));
/** How far the speed is from the track's own, as a mixer's tempo fader reads. */
const speedLabel = computed(() => {
  const percent = (speed.value - 1) * 100;
  return `${percent > 0 ? '+' : '−'}${Math.abs(percent).toFixed(1)}%`;
});

/** Plays at a tempo, by playing faster or slower. */
function setPlayBpm(bpm: number) {
  const own = bpmHere.value;
  if (!own || !Number.isFinite(bpm) || bpm <= 0) return;
  const next = Math.max(SPEED_RANGE[0], Math.min(SPEED_RANGE[1], bpm / own));
  speed.value = Math.abs(next - 1) < 0.0005 ? 1 : next;
}

/** A step faster or slower: 1 BPM, or with Shift a tenth of one. */
function stepPlayBpm(direction: number, event: MouseEvent | KeyboardEvent) {
  typed.value = null;
  if (playBpm.value) setPlayBpm(playBpm.value + direction * (event.shiftKey ? 0.1 : 1));
}

function typePlayBpm(event: Event) {
  const input = event.target as HTMLInputElement;
  setPlayBpm(Number(input.value));
  typed.value = null;
  // What was typed gives way to what it came to: a tempo out of range, or not a number.
  input.value = playBpm.value?.toFixed(2) ?? '';
}

/** Clicking the current rating clears it, as in rekordbox. */
function rate(stars: number) {
  const track = props.track;
  if (!track) return;
  const next = track.rating === stars ? 0 : stars;
  browse.patchTrack(track.id, { rating: next });
  store.edit({ kind: 'setRating', trackId: track.id, stars: next });
}

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
  if (!props.track) return;
  const now = !locked.value;
  change(
    { kind: 'lockAnalysis', trackId: props.track.id, locked: now },
    {
      show: () => {
        if (prep.value) prep.value = { ...prep.value, locked: now };
      },
    },
  );
}

// --- Drawing -----------------------------------------------------------------

const detail = ref<HTMLCanvasElement | null>(null);
const overview = ref<HTMLCanvasElement | null>(null);
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
  // The loop being played, brighter, with its two ends marked.
  const loop = activeLoop.value;
  if (loop && loop.endMs >= startMs && loop.startMs <= endMs) {
    c.fillStyle = 'rgba(255,130,5,0.3)';
    c.fillRect(x(loop.startMs), 0, (loop.endMs - loop.startMs) * pxPerMs, height);
    c.fillStyle = '#ff8205';
    c.fillRect(x(loop.startMs), 0, 2, height);
    c.fillRect(x(loop.endMs) - 2, 0, 2, height);
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
  const loop = activeLoop.value;
  if (loop) {
    c.fillStyle = '#ff8205';
    c.fillRect(at(loop.startMs), height - 3, Math.max(2, at(loop.endMs) - at(loop.startMs)), 3);
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
watch(
  [() => props.track?.id, () => props.track?.bpm, () => props.track?.analysed],
  ([id, bpm, analysed], [wasId, , wasAnalysed]) => {
    // Another track has another tempo too, and is loaded as a whole above.
    if (id !== wasId) return;
    // A first analysis brings waveforms with it.
    if (analysed !== wasAnalysed) return void loadPreparation();
    // A tempo set here is on the row because it was written from here: it's showing already.
    const shown = prep.value?.bpm;
    if (bpm != null && shown != null && Math.abs(bpm - shown) < 0.005) return;
    // The waveforms are the same audio's, whatever its grid: only the grid is read.
    loadPreparation(false);
  },
);
// Another of the analysis's grids tried from the track's details. (A grid
// changed by hand was changed here, and is read once it's written.)
watch(
  () => [props.track?.id, props.track?.grid?.using] as const,
  ([id, using], [wasId, wasUsing]) => {
    if (id && id === wasId && using !== wasUsing && using !== 'hand' && !pending.value) loadPreparation(false);
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
  [position, zoom, palette, waves, prep, playing, durationMs, activeLoop, () => props.track?.id],
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
  window.addEventListener('keyup', onKeyUp, true);
});
onBeforeUnmount(() => {
  cancelAnimationFrame(frame);
  resized.disconnect();
  window.removeEventListener('resize', invalidate);
  window.removeEventListener('keydown', onKey);
  window.removeEventListener('keyup', onKeyUp, true);
  engine.dispose();
});

// One height for everything in the tools' rows: the pads and the loop's two rows are made to it too.
const buttonClass =
  'inline-flex h-6 items-center justify-center rounded bg-surface-2 px-2 text-[11px] whitespace-nowrap transition-colors hover:bg-surface-3 disabled:opacity-40';
const onClass =
  'inline-flex h-6 items-center justify-center rounded bg-accent px-2 text-[11px] whitespace-nowrap text-white transition-colors disabled:opacity-40';
/** A figure in the top bar: a small label over a large value. */
const statClass = 'flex shrink-0 flex-col items-center leading-none';
const statLabelClass = 'mb-1 text-[9px] font-medium tracking-wider text-text-muted uppercase';
const statValueClass = 'font-mono text-base font-semibold text-text-primary';
</script>

<template>
  <section class="flex shrink-0 flex-col gap-1.5 border-b border-border px-4 pb-2 text-xs">
    <div class="flex h-14 items-center gap-3 pt-1">
      <button
        class="flex size-9 shrink-0 items-center justify-center rounded-full bg-surface-3 text-sm hover:bg-accent disabled:opacity-40"
        :disabled="!ready"
        :title="playing ? 'Pause (Space)' : 'Play (Space)'"
        @click="toggle"
      >
        {{ playing ? '❚❚' : '▶' }}
      </button>
      <LibraryCover :path="track?.path ?? null" class="size-11" />
      <div class="min-w-0 flex-1">
        <template v-if="track">
          <p class="truncate text-sm leading-tight font-medium">{{ track.title }}</p>
          <p class="truncate leading-tight text-text-secondary">
            {{ track.artist }}<span v-if="track.album" class="text-text-muted"> · {{ track.album }}</span>
          </p>
        </template>
        <p v-else class="text-text-muted">No track loaded</p>
      </div>
      <template v-if="track">
        <div :class="statClass" class="w-36">
          <span :class="statLabelClass">Time</span>
          <span :class="statValueClass" class="text-sm">
            <template v-if="loading">Decoding…</template>
            <template v-else>{{ timeLabel }}</template>
          </span>
        </div>
        <div v-if="barAndBeat" :class="statClass" class="w-14" title="Bar and beat at the playhead">
          <span :class="statLabelClass">Bar</span>
          <span :class="statValueClass">{{ barAndBeat }}</span>
        </div>
        <div v-if="playBpm" :class="statClass">
          <span :class="statLabelClass" class="relative">
            BPM
            <!-- Out of the flow, beside the label: it comes and goes without moving the tempo under it. -->
            <button
              v-if="speed !== 1"
              class="absolute top-1/2 left-full ml-1 -translate-y-1/2 rounded bg-status-busy/20 px-1 font-mono tracking-normal whitespace-nowrap text-status-busy hover:bg-status-busy/30"
              title="Playing faster or slower than the track's own tempo. Click to go back to it."
              @click="speed = 1"
            >{{ speedLabel }}</button>
          </span>
          <span class="flex items-center gap-1">
            <input
              :value="shown('playBpm', playBpm.toFixed(2))"
              :class="[statValueClass, speed !== 1 ? 'text-status-busy' : '']"
              class="w-[4.75rem] rounded border border-transparent bg-transparent text-center outline-none hover:border-border focus:border-border-active"
              title="The tempo it plays at. Type one, or use the arrows: the track plays faster or slower, and its pitch goes with it, as on a turntable. The track and its grid aren't changed."
              spellcheck="false"
              inputmode="decimal"
              @input="type('playBpm', $event)"
              @change="typePlayBpm"
              @blur="typed = null"
              @keydown.enter="($event.target as HTMLInputElement).blur()"
              @keydown.up.prevent="stepPlayBpm(1, $event)"
              @keydown.down.prevent="stepPlayBpm(-1, $event)"
            />
            <span class="flex flex-col gap-px text-[8px] leading-none text-text-secondary">
              <button
                class="rounded-t bg-surface-2 px-1 py-0.5 hover:bg-surface-3"
                title="Faster by 1 BPM (Shift: 0.1)"
                @click="stepPlayBpm(1, $event)"
              >▲</button>
              <button
                class="rounded-b bg-surface-2 px-1 py-0.5 hover:bg-surface-3"
                title="Slower by 1 BPM (Shift: 0.1)"
                @click="stepPlayBpm(-1, $event)"
              >▼</button>
            </span>
          </span>
        </div>
        <div v-else-if="track.bpm" :class="statClass">
          <span :class="statLabelClass">BPM</span>
          <span :class="statValueClass">{{ track.bpm.toFixed(2) }}</span>
        </div>
        <div v-if="track.key" :class="statClass" class="w-10">
          <span :class="statLabelClass">Key</span>
          <span :class="statValueClass">{{ formatKey(track.key) }}</span>
        </div>
        <div :class="statClass">
          <span :class="statLabelClass">Rating</span>
          <span class="flex text-base leading-none">
            <button
              v-for="star in 5"
              :key="star"
              tabindex="-1"
              :class="star <= track.rating ? 'text-status-busy hover:text-text-primary' : 'text-text-muted/50 hover:text-text-secondary'"
              :title="`${star} ${star === 1 ? 'star' : 'stars'}`"
              @click="rate(star)"
            >{{ star <= track.rating ? '★' : '☆' }}</button>
          </span>
        </div>
      </template>
      <VolumeKnob v-model="volume" label="Master volume" class="size-8" />
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

    <div class="flex min-h-7 flex-wrap items-center justify-center gap-x-3 gap-y-1.5" :class="track ? '' : 'pointer-events-none opacity-40'">
      <div class="flex items-center gap-1">
        <button
          v-for="(letter, slot) in PADS"
          :key="letter"
          class="relative h-6 w-8 rounded pt-0.5 text-[11px] leading-none font-semibold transition-colors disabled:opacity-40"
          :class="hotCues[slot] ? 'text-black' : 'bg-surface-2 text-text-muted hover:bg-surface-3'"
          :style="hotCues[slot] ? { backgroundColor: cueColor(hotCues[slot]!) } : {}"
          :title="hotCues[slot]
            ? `Hot cue ${letter} at ${formatTime(hotCues[slot]!.positionMs)}. Click (${slot + 1}) to go there, Shift-click (Shift+${slot + 1}) to delete.`
            : activeLoop
              ? `Save the loop playing as hot cue ${letter} (${slot + 1})`
              : `Set hot cue ${letter} at the playhead (${slot + 1})`"
          @click="pressPad(slot, $event)"
        >
          <span class="absolute top-0.5 left-0.5 font-mono text-[8px] leading-none font-normal opacity-70">{{ slot + 1 }}</span>
          {{ letter }}
        </button>
      </div>
      <button
        :class="buttonClass"
        :title="activeLoop ? 'Save the loop playing as a memory cue (M)' : 'Set a memory cue at the playhead (M)'"
        aria-label="Set a memory cue"
        @click="setCue(null)"
      >
        <!-- A memory cue as the waveform marks one: a red head on its line. -->
        <svg viewBox="0 0 12 14" class="h-3.5 w-auto" fill="#ea3323" stroke="#ea3323" stroke-linejoin="round" stroke-linecap="round">
          <path d="M1.5 1.5h9L6 7.5z" stroke-width="1.5" />
          <path d="M6 7.5v5.5" stroke-width="1.5" fill="none" />
        </svg>
      </button>
      <button
        :class="quantize ? onClass : buttonClass"
        class="font-semibold"
        :title="quantize
          ? 'Quantize is on: new cues, loops and the downbeat go on the nearest beat. Click (Q) to turn it off.'
          : 'Quantize is off: new cues, loops and the downbeat go exactly at the playhead. Click (Q) to turn it on.'"
        :aria-pressed="quantize"
        @click="quantize = !quantize"
      >
        Q
      </button>
      <!-- The loop: its length above, to start and leave it; halve and double beneath. -->
      <div class="flex h-6 w-14 shrink-0 flex-col gap-px text-[10px]">
        <button
          class="min-h-0 flex-1 rounded-t px-1 leading-none font-medium whitespace-nowrap transition-colors"
          :class="activeLoop ? 'bg-status-busy text-black' : 'bg-surface-2 hover:bg-surface-3'"
          :title="activeLoop
            ? 'Looping. Click to leave the loop. A cue set now is saved as this loop.'
            : `Loop ${loopLabel} from the playhead`"
          :aria-pressed="!!activeLoop"
          @click="toggleLoop"
        >
          {{ loopLabel }}
        </button>
        <div class="flex h-2.5 shrink-0 gap-px text-[7px] leading-none text-text-secondary">
          <button
            class="flex flex-1 items-center justify-center rounded-bl bg-surface-2 hover:bg-surface-3 disabled:opacity-40"
            title="Halve the loop"
            :disabled="loopSize === 0"
            @click="stepLoop(-1)"
          >◀</button>
          <button
            class="flex flex-1 items-center justify-center rounded-br bg-surface-2 hover:bg-surface-3 disabled:opacity-40"
            title="Double the loop"
            :disabled="loopSize === LOOP_SIZES.length - 1"
            @click="stepLoop(1)"
          >▶</button>
        </div>
      </div>

      <button
        :class="gridTools ? onClass : buttonClass"
        :title="gridTools ? 'Hide the beat grid\'s tools (G)' : 'Show the beat grid\'s tools: tempo, downbeat, metronome (G)'"
        aria-label="Beat grid tools"
        :aria-pressed="gridTools"
        @click="gridTools = !gridTools"
      >
        <!-- A beat grid: a bar's four lines, the first taller. -->
        <svg viewBox="0 0 18 12" class="h-3.5 w-auto" fill="none" stroke="currentColor" stroke-linecap="round">
          <path d="M2.5 1v10" stroke-width="2" />
          <path d="M7 4v7M11.5 4v7M16 4v7" stroke-width="1.5" />
        </svg>
      </button>

      <!-- The cues' tools on one line, the beat grid's on the next. -->
      <span v-if="gridTools" class="h-0 basis-full" aria-hidden="true" />
      <div v-if="gridTools" class="flex flex-wrap items-center justify-center gap-x-3 gap-y-1.5">
        <button
          :class="buttonClass"
          class="font-medium"
          :disabled="!canEditGrid"
          :title="quantize
            ? `Make the beat nearest the playhead the first beat of its bar. The grid doesn't move. (${shortcut('B')})`
            : `Make the beat nearest the playhead the first beat of its bar, and slide the grid to put it exactly at the playhead: quantize is off. (${shortcut('B')})`"
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
        <button
          :class="fromHere ? onClass : buttonClass"
          :title="fromHere
            ? 'From the playhead on: grid changes leave the beats before the playhead as they are, for a track that changes tempo. Click to change the whole grid again.'
            : 'Change the grid only from the playhead on, leaving the beats before it: for a track that changes tempo'"
          aria-label="From playhead on"
          :aria-pressed="fromHere"
          @click="fromHere = !fromHere"
        >
          <!-- The playhead, and an arrow on from it. -->
          <svg viewBox="0 0 18 12" class="h-3.5 w-auto" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <path d="M2.5 1v10" stroke="#ff3b30" stroke-width="2" />
            <path d="M6.5 6h9M12 2.5 15.5 6 12 9.5" stroke="currentColor" stroke-width="1.5" />
          </svg>
        </button>
        <div class="flex items-center gap-1">
          <button
            :class="metronome ? onClass : buttonClass"
            :disabled="!beats.length"
            title="Click on every beat while playing, to hear whether the grid sits on the music"
            @click="metronome = !metronome"
          >
            Metronome
          </button>
          <VolumeKnob v-model="clickVolume" label="Metronome volume" class="size-6" :class="beats.length ? '' : 'opacity-40'" />
          <button :class="buttonClass" :disabled="!undoStack.length || waiting || locked" title="Undo the last grid change" @click="undo">
            Undo
          </button>
          <button
            :class="locked ? onClass : buttonClass"
            :disabled="!beats.length"
            :title="locked
              ? 'The grid is locked, here and in rekordbox: it can\'t be changed or analysed over. Click to unlock.'
              : 'Lock the grid, here and in rekordbox, so it can\'t be changed or analysed over'"
            @click="toggleLock"
          >
            {{ locked ? 'Locked' : 'Lock' }}
          </button>
        </div>
      </div>
    </div>

    <!-- Every cue, to name or delete: one line. Short of room, the names narrow; past that it scrolls sideways, with no bar shown. -->
    <ul v-if="cues.length" class="flex gap-x-3 overflow-x-auto whitespace-nowrap [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      <li v-for="cue in cues" :key="cueKeys.get(cue.id) ?? cue.id" class="flex min-w-0 items-center gap-1">
        <button
          class="shrink-0 rounded px-1 text-[10px] font-semibold text-black"
          :style="{ backgroundColor: cueColor(cue) }"
          :title="cue.endMs ? 'Play this loop' : 'Go to this cue'"
          @click="goToCue(cue)"
        >
          {{ cueLabel(cue) }}{{ cue.endMs ? ' ↻' : '' }}
        </button>
        <span class="shrink-0 font-mono text-[10px] text-text-muted">{{ formatTime(cue.positionMs) }}</span>
        <input
          :value="shown(`cue-${cueKeys.get(cue.id) ?? cue.id}`, cue.comment)"
          placeholder="name"
          spellcheck="false"
          class="w-16 min-w-9 rounded border border-transparent bg-transparent px-1 text-[11px] outline-none hover:border-border focus:border-border-active"
          @input="type(`cue-${cueKeys.get(cue.id) ?? cue.id}`, $event)"
          @change="nameCue(cue, $event)"
          @blur="typed = null"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <button
          class="shrink-0 text-text-muted hover:text-status-error"
          title="Delete this cue"
          @click="deleteCue(cue)"
        >✕</button>
      </li>
    </ul>
  </section>
</template>
