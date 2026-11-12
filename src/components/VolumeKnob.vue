<script setup lang="ts">
import { computed } from 'vue';

/** A rotary knob, 0 to 1: dragged up and down, or scrolled, as a mixer's is turned. Whoever uses it gives it its size. */
const model = defineModel<number>({ required: true });
defineProps<{ label: string }>();

/** A full turn of the knob takes this much of a drag. */
const DRAG_PX = 120;
/** The knob turns through this many degrees, from bottom left round to bottom right. */
const SWEEP = 270;
const RADIUS = 13;
const ARC = (2 * Math.PI * RADIUS * SWEEP) / 360;

const clamp = (value: number) => Math.max(0, Math.min(1, value));
let from: { y: number; value: number } | null = null;

function down(event: PointerEvent) {
  // A drag turns the knob: it isn't to select the text it passes over.
  event.preventDefault();
  from = { y: event.clientY, value: model.value };
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function move(event: PointerEvent) {
  if (from) model.value = clamp(from.value + (from.y - event.clientY) / DRAG_PX);
}

function wheel(event: WheelEvent) {
  model.value = clamp(model.value - event.deltaY / 400);
}

const percent = computed(() => Math.round(model.value * 100));
</script>

<template>
  <div
    class="shrink-0 cursor-ns-resize touch-none select-none"
    role="slider"
    aria-valuemin="0"
    aria-valuemax="100"
    :aria-valuenow="percent"
    :aria-label="label"
    :title="`${label}: ${percent}%. Drag up or down, or scroll. Double-click for full.`"
    @pointerdown="down"
    @pointermove="move"
    @pointerup="from = null"
    @pointercancel="from = null"
    @wheel.prevent="wheel"
    @mousedown.prevent
    @dblclick="model = 1"
  >
    <svg viewBox="0 0 32 32" class="size-full">
      <g fill="none" stroke-width="2.5" stroke-linecap="round" transform="rotate(135 16 16)">
        <circle cx="16" cy="16" :r="RADIUS" class="stroke-surface-3" :stroke-dasharray="`${ARC} 100`" />
        <circle
          v-if="model > 0"
          cx="16"
          cy="16"
          :r="RADIUS"
          class="stroke-accent-hover"
          :stroke-dasharray="`${ARC * model} 100`"
        />
      </g>
      <circle cx="16" cy="16" r="9" class="fill-surface-3" />
      <line
        x1="16"
        y1="8.5"
        x2="16"
        y2="13"
        class="stroke-text-primary"
        stroke-width="2"
        stroke-linecap="round"
        :transform="`rotate(${model * SWEEP - SWEEP / 2} 16 16)`"
      />
    </svg>
  </div>
</template>
