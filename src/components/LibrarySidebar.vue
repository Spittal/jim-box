<script setup lang="ts">
import { computed, type ComponentPublicInstance } from 'vue';
import { storeToRefs } from 'pinia';
import PlaylistTree from '@/components/PlaylistTree.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import { useBrowseStore } from '@/stores/browse';
import { useLibrariesStore } from '@/stores/libraries';

/**
 * The sidebar: the master library's playlists at the top, and below them,
 * splitting the height, each library opened beside it (a USB stick,
 * another master.db). At its foot, opening another.
 */
const libraries = useLibrariesStore();
const { others, plugged, moving, transfer, transferPercent, secondsLeft } = storeToRefs(libraries);

const STAGES: Record<string, string> = {
  matching: 'Finding the tracks you have',
  copying: 'Copying',
  adding: 'Adding them to the collection',
  carrying: 'Bringing over grids, cues and ratings',
  placing: 'Filling the playlists',
};

const gigabytes = (bytes: number) => (bytes / 1e9).toFixed(bytes < 1e10 ? 1 : 0);

/** The stage, and how far through it the move is. */
const transferLine = computed(() => {
  const p = transfer.value;
  if (!p) return moving.value;
  const parts = [STAGES[p.stage] ?? p.stage];
  if (p.total) parts[0] += ` ${Math.min(p.done + (p.stage === 'copying' ? 0 : 1), p.total).toLocaleString()} of ${p.total.toLocaleString()}`;
  if (p.stage === 'copying' && p.bytesTotal) parts.push(`${gigabytes(p.bytesDone)} of ${gigabytes(p.bytesTotal)} GB`);
  return parts.join(' · ');
});

/** How long the copying has left, roughly. */
const timeLeft = computed(() => {
  const s = secondsLeft.value;
  if (s === null) return null;
  if (s < 60) return 'under a minute left';
  const minutes = Math.round(s / 60);
  return minutes < 90 ? `about ${minutes} min left` : `about ${Math.round(minutes / 6) / 10} h left`;
});
const browse = useBrowseStore();

/** The master first, then the others in the order opened. */
const sections = computed<(string | null)[]>(() => [null, ...others.value.map((o) => o.id)]);
/** Each says which library it is once there's more than one. */
const titled = computed(() => others.value.length > 0);

type Tree = InstanceType<typeof PlaylistTree>;
const trees = new Map<string | null, Tree>();
function keep(id: string | null, el: Element | ComponentPublicInstance | null) {
  if (el) trees.set(id, el as Tree);
  else trees.delete(id);
}
/** The tree whose list is shown, which the keys are for. */
const active = () => trees.get(browse.library);

/** The keyboard, when the pane has it (see useDetailKeys.ts): for the tree being browsed. */
function onKey(event: KeyboardEvent) {
  active()?.onKey(event);
}

const buttonClass =
  'flex items-center gap-1.5 rounded leading-none px-1.5 py-1 text-[11px] whitespace-nowrap text-text-muted hover:bg-surface-2 hover:text-text-primary';
const OPEN_ICON = 'M1.5 3.5h4.5l1.5 1.5h7v8h-13zM8 11V7.5M6.3 9.2L8 7.5l1.7 1.7';
</script>

<template>
  <div
    data-pane="playlists"
    tabindex="-1"
    class="flex w-56 shrink-0 flex-col border-r border-border outline-none"
    @keydown="onKey"
  >
    <PlaylistTree
      v-for="id in sections"
      :key="id ?? 'master'"
      :ref="(el) => keep(id, el)"
      :library="id"
      :titled="titled"
      class="min-h-0 flex-1 basis-0"
    />

    <!-- Drives plugged in with a library on them, to open. -->
    <ul v-if="plugged.length" class="shrink-0 border-t border-border py-1 text-[11px]">
      <li v-for="drive in plugged" :key="drive.path" class="flex items-center gap-1.5 py-0.5 pr-1.5 pl-3">
        <span class="size-1.5 shrink-0 rounded-full bg-accent" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate text-text-secondary" :title="drive.path">
          {{ drive.name }}
          <span class="text-text-muted">{{ drive.kind === 'stick' ? 'plugged in' : 'has a library' }}</span>
        </span>
        <button :class="buttonClass" class="text-accent" @click="libraries.open(drive.path)">Open</button>
      </li>
    </ul>
    <div v-if="moving" class="flex shrink-0 flex-col gap-1 border-t border-border px-3 py-1.5 text-[11px]" :title="moving">
      <p class="truncate text-text-secondary">{{ transferLine }}</p>
      <ProgressBar :percent="transferPercent ?? 100" :class="transferPercent === null ? 'animate-pulse' : ''" />
      <p class="flex gap-1 text-text-muted">
        <span class="min-w-0 flex-1 truncate">{{ transfer?.title }}</span>
        <span v-if="timeLeft" class="shrink-0">{{ timeLeft }}</span>
      </p>
      <p v-if="transfer?.stage === 'copying'" class="text-text-muted">Keep Jim Box open and the stick in until this is done.</p>
    </div>

    <div class="flex shrink-0 items-center border-t border-border p-1.5">
      <button
        :class="buttonClass"
        title="Open a USB stick or another rekordbox library beside yours: pick the stick, or a folder with a master.db in it"
        @click="libraries.open()"
      >
        <svg viewBox="0 0 16 16" class="block size-3.5 shrink-0" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path :d="OPEN_ICON" />
        </svg>
        Open library…
      </button>
    </div>
  </div>
</template>
