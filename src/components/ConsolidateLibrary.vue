<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useLibraryStore } from '@/stores/library';
import { useSettingsStore } from '@/stores/settings';
import { useUiStore } from '@/stores/ui';
import { confirm } from '@/composables/useConfirm';
import { formatBytes } from '@/composables/useFormat';
import type { RelocateReport } from '@/types/library';

/**
 * Brings collection tracks together: the places ticked are emptied into the
 * library folder chosen, after showing what would move.
 */
const libraryStore = useLibraryStore();
const settingsStore = useSettingsStore();
const { settings } = storeToRefs(settingsStore);
const { plan } = storeToRefs(libraryStore);
const ui = useUiStore();

const root = ref<HTMLElement | null>(null);
/** The library folder to move into; the primary one until another is picked. */
const target = ref('');
/** Paths of the places to move out of. */
const picked = ref<string[]>([]);
const moving = ref(false);
const report = ref<RelocateReport | null>(null);
const error = ref<string | null>(null);

const count = (n: number, one: string, many: string) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

const targets = computed(() => (settings.value ? [settings.value.musicDir, ...settings.value.libraryDirs] : []));
/** Everywhere with tracks that could move into the target. */
const sources = computed(() => plan.value?.places.filter((place) => place.tracks && place.path !== target.value) ?? []);
const chosen = computed(() => sources.value.filter((place) => picked.value.includes(place.path)));
const total = computed(() => ({
  tracks: chosen.value.reduce((sum, place) => sum + place.tracks, 0),
  bytes: chosen.value.reduce((sum, place) => sum + place.bytes, 0),
}));

// Keep the destination one of the library folders, and the primary by default.
watch(
  targets,
  (dirs) => {
    if (!dirs.includes(target.value)) target.value = dirs[0] ?? '';
  },
  { immediate: true },
);

async function look() {
  try {
    await libraryStore.loadPlan();
  } catch (e) {
    error.value = String(e);
  }
}

async function consolidate() {
  const yes = await confirm({
    title: `Move ${count(total.value.tracks, 'file', 'files')} (${formatBytes(total.value.bytes)})?`,
    message: `They are moved into ${target.value} and filed by the paths set for the library. The tracks keep their cues, beat grids and playlists, and nothing is overwritten.`,
    confirm: 'Move',
  });
  if (!yes) return;
  moving.value = true;
  error.value = null;
  report.value = null;
  try {
    report.value = await libraryStore.consolidate(chosen.value.map((place) => place.path), target.value);
    picked.value = [];
    // Library folders that were emptied have come off the list.
    await settingsStore.load();
  } catch (e) {
    error.value = String(e);
  } finally {
    moving.value = false;
    await look();
  }
}

/** File ▸ Consolidate Library… lands here. */
async function showIfAsked() {
  if (!ui.consolidate) return;
  ui.consolidate = false;
  await nextTick();
  root.value?.scrollIntoView({ block: 'center' });
}

onMounted(() => {
  look();
  showIfAsked();
});
watch(() => ui.consolidate, showIfAsked);
// The library folders changed: what's in and out of them did too.
watch(targets, look);
</script>

<template>
  <div ref="root" class="flex flex-col gap-2">
    <span class="text-xs text-text-secondary">Consolidate</span>
    <span v-if="!plan" class="text-xs text-text-muted">Looking through your rekordbox collection…</span>
    <template v-else>
      <label class="flex items-center gap-2 text-xs text-text-muted">
        Into
        <select
          v-model="target"
          :disabled="moving"
          class="min-w-0 flex-1 truncate rounded border border-border bg-surface-1 px-2 py-1.5 font-mono text-xs outline-none focus:border-accent"
        >
          <option v-for="(dir, index) in targets" :key="dir" :value="dir">{{ index === 0 ? '★ ' : '' }}{{ dir }}</option>
        </select>
      </label>
      <span v-if="!sources.length" class="text-xs text-text-muted">
        Every track in your rekordbox collection that can be moved is in this folder already.
      </span>
      <template v-else>
        <span class="text-xs text-text-muted">Move the tracks kept in:</span>
        <ul class="divide-y divide-border rounded border border-border">
          <li v-for="place in sources" :key="place.path">
            <label class="flex cursor-pointer items-center gap-2 px-3 py-1.5">
              <input v-model="picked" type="checkbox" :value="place.path" :disabled="moving" class="shrink-0 accent-accent" />
              <code class="min-w-0 flex-1 truncate font-mono text-xs text-text-secondary" :title="place.path">{{ place.path }}</code>
              <span v-if="place.library" class="shrink-0 text-[11px] text-text-muted">library folder</span>
              <span class="shrink-0 text-xs text-text-muted">
                {{ count(place.tracks, 'track', 'tracks') }} · {{ formatBytes(place.bytes) }}
              </span>
            </label>
          </li>
        </ul>
        <span class="text-xs text-text-muted">
          Their files are moved and filed by the paths below. The tracks keep their cues, beat grids
          and playlists, and nothing is overwritten. A library folder whose tracks all move comes off
          the list of library folders; if it was the starred one, the folder you move into takes the star.
        </span>
        <div class="flex items-center gap-2">
          <button
            :disabled="moving || plan.rekordboxRunning || !total.tracks"
            class="rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
            @click="consolidate"
          >
            {{ moving ? 'Moving…' : 'Consolidate…' }}
          </button>
          <span v-if="plan.rekordboxRunning" class="text-xs text-text-muted">
            Quit rekordbox first (and its agent in the menu bar).
          </span>
        </div>
      </template>
      <span v-if="plan.exported" class="text-xs text-text-muted">
        {{ count(plan.exported, 'track is', 'tracks are') }} on an exported rekordbox drive and
        {{ plan.exported === 1 ? 'stays' : 'stay' }} there: moving the files would break the drive.
      </span>
      <span v-if="plan.missing" class="text-xs text-text-muted">
        {{ count(plan.missing, 'track points', 'tracks point') }} at a file that can't be found (is a
        drive disconnected?) and {{ plan.missing === 1 ? 'is' : 'are' }} left as {{ plan.missing === 1 ? 'it is' : 'they are' }}.
      </span>
    </template>
    <p v-if="report" class="text-xs text-status-ok">Moved {{ count(report.moved, 'file', 'files') }}.</p>
    <div v-if="report?.failed.length" class="error-box">
      {{ report.failed.length }} couldn't be moved and {{ report.failed.length === 1 ? 'is' : 'are' }} still where
      {{ report.failed.length === 1 ? 'it was' : 'they were' }}:
      <ul class="mt-1 list-disc pl-4 font-mono">
        <li v-for="f in report.failed" :key="f" class="break-all">{{ f }}</li>
      </ul>
    </div>
    <p v-if="error" class="error-box">{{ error }}</p>
  </div>
</template>
