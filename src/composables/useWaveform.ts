import { ref, watch } from 'vue';
import type { Cue } from '@/types/collection';

/**
 * rekordbox's waveforms and cue markers, drawn the way rekordbox draws them:
 * what the player and the track list's previews share.
 */

type Rgb = [number, number, number];
/** 3Band: one colour for each combination of bands reaching a point. */
const LOW: Rgb = [0, 85, 225];
const MID: Rgb = [255, 166, 0];
const HIGH: Rgb = [255, 255, 255];
const ALL: Rgb = [245, 235, 215];
/** Indexed by low = 1, mid = 2, high = 4, added together. */
const BAND_COLORS = [LOW, LOW, MID, [180, 105, 10], HIGH, [210, 220, 250], [255, 240, 215], ALL].map(
  (c) => `rgb(${c[0]},${c[1]},${c[2]})`,
);

export type Palette = '3band' | 'rgb' | 'blue';
/** The waveform styles, and the kind of waveform each is drawn from. */
export const PALETTES: { id: Palette; label: string; kind: number }[] = [
  { id: '3band', label: '3Band', kind: 7 },
  { id: 'rgb', label: 'RGB', kind: 5 },
  { id: 'blue', label: 'Blue', kind: 3 },
];

/** The style chosen in the player, kept between runs; the track list follows it. */
export const chosenPalette = ref<Palette>((localStorage.getItem('player.palette') as Palette) || '3band');
watch(chosenPalette, (palette) => localStorage.setItem('player.palette', palette));

// rekordbox's own cue colours, as rbxport measured them from it.
const CUE_COLOR = { hotCue: '#3ceb50', loop: '#ff8205', memoryCue: '#ea3323' };

export function cueColor(cue: Cue) {
  if (cue.slot === null) return CUE_COLOR.memoryCue;
  // A hot cue's own colour in rekordbox, or its pad's.
  return cue.color ?? (cue.endMs ? CUE_COLOR.loop : CUE_COLOR.hotCue);
}

/** A hot cue's pad letter; M for a memory cue. */
export function cueLabel(cue: Cue) {
  return cue.slot === null ? 'M' : String.fromCharCode(65 + cue.slot);
}

/** Fits a canvas's pixels to its size on screen. */
export function fit(canvas: HTMLCanvasElement): [number, number, CanvasRenderingContext2D] {
  const ratio = window.devicePixelRatio || 1;
  const { clientWidth: width, clientHeight: height } = canvas;
  if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
  }
  const context = canvas.getContext('2d')!;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  return [width, height, context];
}

/** A colour from its three channels with the strongest at full, as a CDJ draws RGB waveforms. */
function rgbOf(r: number, g: number, b: number): string {
  const peak = Math.max(r, g, b);
  if (!peak) return 'rgb(0,0,0)';
  const scale = 255 / peak;
  return `rgb(${Math.round(r * scale)},${Math.round(g * scale)},${Math.round(b * scale)})`;
}

function blend(a: Rgb, b: Rgb, t: number): string {
  const c = (n: number) => Math.round(a[n] + (b[n] - a[n]) * t);
  return `rgb(${c(0)},${c(1)},${c(2)})`;
}

// Every colour a slice can be, worked out once: slices are drawn by the
// thousand each frame.
/** RGB: by the nine bits of red, green and blue. */
const RGB_COLORS = Array.from({ length: 512 }, (_, bits) => rgbOf(bits >> 6, (bits >> 3) & 7, bits & 7));
/** Blue: by the three bits of whiteness. */
const BLUE_COLORS = Array.from({ length: 8 }, (_, shade) => blend(LOW, ALL, shade / 7));

/** A waveform to draw: `columns` columns of rekordbox's kind 7, 5 or 3. */
export interface Wave {
  kind: number;
  data: Uint8Array;
  columns: number;
}

/** One pixel-wide bar, `height` (0 to 1) of `reach` tall. */
function bar(
  c: CanvasRenderingContext2D,
  px: number,
  base: number,
  reach: number,
  mirrored: boolean,
  height: number,
  color: string,
) {
  if (height <= 0) return;
  const tall = Math.max(1, height * reach);
  c.fillStyle = color;
  c.fillRect(px, base - tall, 1, mirrored ? tall * 2 : tall);
}

/**
 * Draws one pixel-wide slice of waveform from the loudest of the columns
 * `from` to `to`: `reach` pixels up from `base`, and as far down when `mirrored`.
 */
export function drawSlice(
  c: CanvasRenderingContext2D,
  wave: Wave,
  px: number,
  from: number,
  to: number,
  base: number,
  reach: number,
  mirrored: boolean,
) {
  const { kind, data } = wave;
  const first = Math.max(0, from);
  const last = Math.min(wave.columns, Math.max(to, from + 1));
  if (first >= last) return;

  if (kind === 7) {
    let low = 0;
    let mid = 0;
    let high = 0;
    for (let i = first; i < last; i++) {
      const at = i * 3;
      if (data[at] > low) low = data[at];
      if (data[at + 1] > mid) mid = data[at + 1];
      if (data[at + 2] > high) high = data[at + 2];
    }
    // The three bands, shortest first (v for value, b for band).
    let v0 = low;
    let b0 = 1;
    let v1 = mid;
    let b1 = 2;
    let v2 = high;
    let b2 = 4;
    let swap: number;
    if (v0 > v1) {
      swap = v0; v0 = v1; v1 = swap;
      swap = b0; b0 = b1; b1 = swap;
    }
    if (v1 > v2) {
      swap = v1; v1 = v2; v2 = swap;
      swap = b1; b1 = b2; b2 = swap;
    }
    if (v0 > v1) {
      swap = v0; v0 = v1; v1 = swap;
      swap = b0; b0 = b1; b1 = swap;
    }
    // From the outside in: each band's stretch covers the last, so a point
    // ends up in the colour of exactly the bands that reach it.
    bar(c, px, base, reach, mirrored, Math.min(v2, 127) / 127, BAND_COLORS[b2]);
    bar(c, px, base, reach, mirrored, Math.min(v1, 127) / 127, BAND_COLORS[b1 | b2]);
    bar(c, px, base, reach, mirrored, Math.min(v0, 127) / 127, BAND_COLORS[7]);
  } else if (kind === 5) {
    let best = 0;
    for (let i = first; i < last; i++) {
      const word = (data[i * 2] << 8) | data[i * 2 + 1];
      if (((word >> 2) & 31) >= ((best >> 2) & 31)) best = word;
    }
    bar(c, px, base, reach, mirrored, ((best >> 2) & 31) / 31, RGB_COLORS[(best >> 7) & 511]);
  } else {
    let best = 0;
    for (let i = first; i < last; i++) if ((data[i] & 31) >= (best & 31)) best = data[i];
    bar(c, px, base, reach, mirrored, (best & 31) / 31, BLUE_COLORS[best >> 5]);
  }
}

/** A marker's head: a triangle pointing down from the top, or up from the bottom. */
export function head(c: CanvasRenderingContext2D, x: number, edge: number, size: number, down: boolean) {
  c.beginPath();
  c.moveTo(x - size, edge);
  c.lineTo(x + size, edge);
  c.lineTo(x, edge + (down ? size * 1.4 : -size * 1.4));
  c.closePath();
  c.fill();
}
