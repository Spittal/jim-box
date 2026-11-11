<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { formatDuration } from '@/composables/useFormat';
import { cueColor, cueLabel, drawSlice, fit, head } from '@/composables/useWaveform';
import type { TrackPreview } from '@/types/collection';

/**
 * A track's whole waveform in a row of the track list, with its cues along
 * the top as rekordbox's Preview column has them: hot cues in their pads'
 * colours, memory cues red, and a bar over each loop.
 */
const props = defineProps<{ preview: TrackPreview | null }>();

const canvas = ref<HTMLCanvasElement | null>(null);
/** Room above the waveform for the cues' heads. */
const MARKERS = 5;

function draw() {
  if (!canvas.value) return;
  const [width, height, c] = fit(canvas.value);
  c.clearRect(0, 0, width, height);
  const preview = props.preview;
  if (!preview) return;

  if (preview.kind) {
    const { kind, data, columns } = preview;
    // A slice for each of the screen's own pixels, which are finer than the layout's.
    const slices = canvas.value.width;
    c.save();
    c.scale(width / slices, 1);
    for (let px = 0; px < slices; px++) {
      const from = Math.floor((px * columns) / slices);
      const to = Math.floor(((px + 1) * columns) / slices);
      drawSlice(c, { kind, data, columns }, px, from, to, height, height - MARKERS, false);
    }
    c.restore();
  }

  const total = preview.durationMs || 1;
  const at = (ms: number) => Math.min(width - 1, Math.round((ms / total) * width));
  for (const cue of preview.cues) {
    const x = at(cue.positionMs);
    c.fillStyle = cueColor(cue);
    if (cue.endMs) c.fillRect(x, 0, Math.max(1, at(cue.endMs) - x), 1.5);
    // A hot cue's line runs down through the waveform; a memory cue is only its head.
    if (cue.slot !== null) {
      c.globalAlpha = 0.55;
      c.fillRect(x, 0, 1, height);
      c.globalAlpha = 1;
    }
    head(c, x + 0.5, 0, 3, true);
  }
}

// Drawn once it's laid out, and again whenever its column is resized.
const resized = new ResizeObserver(draw);
onMounted(() => canvas.value && resized.observe(canvas.value));
onUnmounted(() => resized.disconnect());
watch(() => props.preview, draw);

/** The cues in words, for hovering. */
const title = computed(() => {
  const cues = props.preview?.cues ?? [];
  if (!cues.length) return undefined;
  return cues
    .map((cue) => {
      const kind = cue.slot === null ? 'Memory' : `Hot cue ${cueLabel(cue)}`;
      const time = formatDuration(Math.floor(cue.positionMs / 1000));
      return `${kind}${cue.endMs ? ' loop' : ''}  ${time}${cue.comment ? `  ${cue.comment}` : ''}`;
    })
    .join('\n');
});
</script>

<template>
  <canvas ref="canvas" class="h-5.5 rounded-sm bg-black/40" :title="title" />
</template>
