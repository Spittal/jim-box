import type { Beat, GridChange } from '@/types/collection';

/**
 * Beat-grid changes worked out here as the backend works them out (rbxport's
 * `rbl_anlz::grid`), so the player can show one the moment it's asked for,
 * ahead of it being written. Whatever is written is read back afterwards and
 * shown in its place.
 */

/** The index of the beat nearest a time, the earlier of two as near; -1 without beats. */
export function nearestBeatIndex(beats: Beat[], ms: number): number {
  if (!beats.length) return -1;
  let lo = 0;
  let hi = beats.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (beats[mid][0] < ms) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 && ms - beats[lo - 1][0] <= beats[lo][0] - ms ? lo - 1 : lo;
}

/** The place in the bar, 1 to 4, of the beat `offset` beats after a bar's first. */
const place = (offset: number) => (((offset % 4) + 4) % 4) + 1;

/** Rounds to the nearest whole number, halves to the even one; never below 0. */
function rounded(value: number): number {
  const up = Math.round(value);
  return Math.max(0, Math.abs(value % 1) === 0.5 && up % 2 !== 0 ? up - 1 : up);
}

/** Keeps a grid within the track, and carries it on to both of its ends. */
function fit(grid: Beat[], endMs: number): Beat[] {
  const beats = grid.filter((beat) => beat[0] <= endMs);
  if (!beats.length) return beats;
  if (beats.length === 1) {
    const first = beats[0];
    if (first[0] === 0) return beats;
    for (;;) {
      const last = beats[beats.length - 1];
      if (last[0] + first[0] > endMs) break;
      beats.push([last[0] + first[0], place(last[1]), last[2]]);
    }
    return beats;
  }
  const lead = Math.max(0, beats[1][0] - beats[0][0]);
  if (lead > 0 && beats[0][0] >= lead) {
    const first = beats[0];
    beats.unshift([first[0] - lead, place(first[1] - 2), first[2]]);
  }
  for (;;) {
    const last = beats[beats.length - 1];
    const interval = Math.max(0, last[0] - beats[beats.length - 2][0]);
    if (!interval || last[0] + interval > endMs) break;
    beats.push([last[0] + interval, place(last[1]), last[2]]);
  }
  return beats;
}

/** Slides the beats from `start` on; ones pushed off the track go. */
function moved(beats: Beat[], start: number, by: number, endMs: number): Beat[] {
  const out: Beat[] = [];
  beats.forEach((beat, i) => {
    const time = beat[0] + (i >= start ? by : 0);
    if (time >= 0 && time <= endMs) out.push([time, beat[1], beat[2]]);
  });
  return fit(out, endMs);
}

/**
 * The grid a change makes of `beats`: all of it, or with `fromMs` the beats
 * from the one nearest it on. `durationMs` is the track's length as the
 * library has it. Null for a change that isn't worked out here, or that the
 * backend would refuse: those are to be sent and waited for.
 */
export function editedGrid(
  beats: Beat[],
  change: GridChange,
  fromMs: number | null,
  durationMs: number,
): Beat[] | null {
  if (change.kind === 'restore') return change.beats;
  if (!beats.length) return null;
  const endMs = durationMs || beats[beats.length - 1][0];
  const start = fromMs === null ? 0 : nearestBeatIndex(beats, fromMs);

  switch (change.kind) {
    case 'nudge':
      return moved(beats, start, change.ms, endMs);
    case 'align':
    case 'downbeat': {
      const at = nearestBeatIndex(beats, change.timeMs);
      const out = moved(beats, start, change.timeMs - beats[at][0], endMs);
      if (change.kind === 'downbeat') {
        const first = nearestBeatIndex(out, change.timeMs);
        out.forEach((beat, i) => (beat[1] = place(i - first)));
      }
      return out;
    }
    case 'stretch': {
      let target = nearestBeatIndex(beats, change.timeMs);
      if (target === start && change.timeMs >= beats[start][0]) target++;
      // Nothing to stretch between: the grid stays as it is.
      if (target <= start || target >= beats.length) return beats;
      const by = Math.max(-120, Math.min(120, change.byMs));
      const interval = (beats[target][0] + by - beats[start][0]) / (target - start);
      if (interval < 60_000 / 499 || interval > 1500) return null;
      const tempo = rounded(6_000_000 / interval);
      const anchor = beats[start][0];
      const out = beats.map<Beat>((beat, i) =>
        i < start ? [beat[0], beat[1], beat[2]] : [anchor + rounded((i - start) * interval), beat[1], tempo],
      );
      return fit(out, endMs);
    }
    default:
      return null;
  }
}

/** Whether two grids are the same, beat for beat. */
export function sameGrid(a: Beat[], b: Beat[]): boolean {
  return a.length === b.length && a.every((beat, i) => beat[0] === b[i][0] && beat[1] === b[i][1] && beat[2] === b[i][2]);
}
