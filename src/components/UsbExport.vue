<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import ProgressBar from '@/components/ProgressBar.vue';
import { asking } from '@/composables/useConfirm';
import { formatBytes } from '@/composables/useFormat';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useUiStore } from '@/stores/ui';
import { useUsbStore } from '@/stores/usb';
import type { PlaylistNode } from '@/types/collection';
import type { UsbConvert, UsbDevice, UsbEstimate, UsbStage } from '@/types/usb';

/**
 * Export to USB, in a dialog over the window: a stick is made to hold the
 * playlists ticked, for players to read. It can be closed while the export
 * runs; the bar under the collection says how it's going.
 */
const ui = useUiStore();
const usb = useUsbStore();
const { devices, looking, exporting, progress, percent, stopping, report, error } = storeToRefs(usb);
const { playlists } = storeToRefs(useCollectionStore());

// --- The stick ----------------------------------------------------------------

const path = ref<string | null>(null);
const device = computed(() => devices.value.find((d) => d.path === path.value) ?? null);

function describe(d: UsbDevice): string {
  const parts = [];
  if (d.totalBytes) parts.push(`${formatBytes(d.freeBytes)} free of ${formatBytes(d.totalBytes)}`);
  if (d.fileSystem) parts.push(d.fileSystem);
  if (d.holds) {
    const { tracks, playlists: lists, ours } = d.holds;
    parts.push(
      `${tracks} ${tracks === 1 ? 'track' : 'tracks'} in ${lists} ${lists === 1 ? 'playlist' : 'playlists'}${ours ? '' : ', written by rekordbox'}`,
    );
  } else parts.push('nothing for players yet');
  return parts.join(' · ');
}

/** Players read FAT32 (and Mac ones HFS+); exFAT only the newest. */
const fileSystemNote = computed(() => {
  const fs = device.value?.fileSystem.toLowerCase() ?? '';
  if (!fs || fs.includes('fat32') || fs.includes('msdos') || fs.includes('hfs') || fs === 'vfat') return '';
  return `This stick is formatted as ${device.value?.fileSystem}. Most players read only FAT32, so check yours reads it.`;
});

// --- The playlists ------------------------------------------------------------

/** The playlists ticked, by id. Folders aren't in it: one is ticked when what's under it is. */
const picked = ref(new Set<string>());

/** The playlists under a folder, at any depth. */
function leavesUnder(folder: PlaylistNode): string[] {
  const rows = playlists.value;
  const leaves = [];
  for (let i = rows.indexOf(folder) + 1; i < rows.length && rows[i].depth > folder.depth; i++) {
    if (rows[i].kind !== 'folder') leaves.push(rows[i].id);
  }
  return leaves;
}

function leavesOf(node: PlaylistNode): string[] {
  return node.kind === 'folder' ? leavesUnder(node) : [node.id];
}

/** Whether a row is ticked: for a folder, 'some' when only part of it is. */
function ticked(node: PlaylistNode): boolean | 'some' {
  const leaves = leavesOf(node);
  const on = leaves.filter((id) => picked.value.has(id)).length;
  return on === 0 ? false : on === leaves.length ? true : 'some';
}

function toggle(node: PlaylistNode) {
  const leaves = leavesOf(node);
  const next = new Set(picked.value);
  if (ticked(node) === true) leaves.forEach((id) => next.delete(id));
  else leaves.forEach((id) => next.add(id));
  picked.value = next;
}

/** The ticked playlists, in the tree's order. */
const chosen = computed(() => playlists.value.filter((p) => p.kind !== 'folder' && picked.value.has(p.id)).map((p) => p.id));

/** Starts from what the stick was last written with; a stick new to this library keeps what's ticked. */
watch(device, (now, before) => {
  if (!now || now.path === before?.path) return;
  const known = new Set(playlists.value.map((p) => p.id));
  const held = now.playlistIds.filter((id) => known.has(id));
  if (held.length) picked.value = new Set(held);
});

// --- How ----------------------------------------------------------------------

const convert = ref<UsbConvert>((localStorage.getItem('usb.convert') as UsbConvert) || null);
const ejectAfter = ref(localStorage.getItem('usb.eject') === 'true');
watch(convert, (to) => (to ? localStorage.setItem('usb.convert', to) : localStorage.removeItem('usb.convert')));
watch(ejectAfter, (on) => localStorage.setItem('usb.eject', String(on)));

// --- What it would do ---------------------------------------------------------

const estimate = ref<UsbEstimate | null>(null);
const estimateError = ref<string | null>(null);
const estimating = ref(false);
let estimateTimer: ReturnType<typeof setTimeout> | undefined;
let estimateAsked = 0;

function reestimate() {
  clearTimeout(estimateTimer);
  const mine = ++estimateAsked;
  estimate.value = null;
  estimateError.value = null;
  estimating.value = false;
  const to = path.value;
  if (!to || !chosen.value.length || exporting.value) return;
  estimating.value = true;
  // Ticking several in a row asks once.
  estimateTimer = setTimeout(async () => {
    try {
      const found = await usb.estimate(to, chosen.value, convert.value);
      if (mine === estimateAsked) estimate.value = found;
    } catch (e) {
      if (mine === estimateAsked) estimateError.value = String(e);
    } finally {
      if (mine === estimateAsked) estimating.value = false;
    }
  }, 400);
}
watch([path, chosen, convert, exporting], reestimate);

const tooBig = computed(() => {
  const e = estimate.value;
  return !!e && e.freeBytes !== null && e.neededBytes > e.freeBytes;
});

const summary = computed(() => {
  const e = estimate.value;
  if (!e) return '';
  const plural = (n: number) => `${n} ${n === 1 ? 'track' : 'tracks'}`;
  const copies = e.tracksNew + e.tracksChanged;
  const parts = [plural(e.tracks)];
  if (copies) parts.push(`${copies} to copy (${e.approximate ? 'about ' : ''}${formatBytes(e.copyBytes)})`);
  else parts.push('all on the stick already');
  if (e.tracksRemoved) parts.push(`${e.tracksRemoved} to take off`);
  if (e.tracksMissing) parts.push(`${e.tracksMissing} left out, their files are missing`);
  return parts.join(' · ');
});

// --- Going ahead --------------------------------------------------------------

const ready = computed(() => !!path.value && chosen.value.length > 0 && !tooBig.value && !exporting.value);

function start() {
  if (!ready.value || !path.value) return;
  void usb.run(path.value, chosen.value, convert.value, ejectAfter.value);
}

const STAGES: Record<UsbStage, string> = {
  preparing: 'Reading the library',
  checking: 'Checking what’s on the stick',
  copying: 'Copying',
  converting: 'Converting',
  database: 'Writing the stick’s library',
  verifying: 'Checking the export',
  publishing: 'Finishing',
  ejecting: 'Ejecting',
};

const going = computed(() => {
  const p = progress.value;
  if (stopping.value) return 'Stopping…';
  if (!p) return 'Starting…';
  const count = p.total ? ` ${Math.min(p.done + 1, p.total)} of ${p.total}` : '…';
  return `${STAGES[p.stage] ?? p.stage}${count}`;
});

const outcome = computed(() => {
  const r = report.value;
  if (!r) return '';
  const plural = (n: number) => `${n} ${n === 1 ? 'track' : 'tracks'}`;
  if (r.upToDate) return `The stick already held these ${plural(r.tracks)}. Nothing needed writing.`;
  const did = [];
  if (r.tracksAdded) did.push(`${r.tracksAdded} added`);
  if (r.tracksUpdated) did.push(`${r.tracksUpdated} updated`);
  if (r.removed) did.push(`${r.removed} taken off`);
  if (r.converted) did.push(`${r.converted} converted`);
  const lists = `${r.playlists} ${r.playlists === 1 ? 'playlist' : 'playlists'}`;
  return `The stick holds ${plural(r.tracks)} in ${lists}${did.length ? `: ${did.join(', ')}` : ''}.`;
});

// --- The dialog ---------------------------------------------------------------

function close() {
  ui.usb = false;
}

function onKey(event: KeyboardEvent) {
  // A question over this one takes the key first.
  if (event.key === 'Escape' && !event.defaultPrevented && !asking.value) close();
}

/** A stick plugged in while the app was in the background shows on coming back. */
function onFocus() {
  if (!exporting.value) void usb.load();
}

onMounted(async () => {
  window.addEventListener('keydown', onKey);
  window.addEventListener('focus', onFocus);
  // The playlists being shown are the ones offered, until a stick says otherwise.
  const browse = useBrowseStore();
  picked.value = new Set(
    browse.lists.flatMap((id) => {
      const node = playlists.value.find((p) => p.id === id);
      return node ? leavesOf(node) : [];
    }),
  );
  path.value = exporting.value;
  await usb.load();
  if (!device.value) path.value = devices.value[0]?.path ?? null;
});
onUnmounted(() => {
  window.removeEventListener('keydown', onKey);
  window.removeEventListener('focus', onFocus);
  clearTimeout(estimateTimer);
});

// A stick pulled out is no longer the one chosen.
watch(devices, (now) => {
  if (path.value && !now.some((d) => d.path === path.value)) path.value = now[0]?.path ?? null;
});
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-6" @click.self="close">
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Export to USB"
        class="flex h-[40rem] max-h-full w-full max-w-2xl flex-col overflow-hidden rounded-lg border border-border bg-surface-0 shadow-xl"
      >
        <header class="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
          <h1 class="text-sm font-medium">Export to USB</h1>
          <button
            class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
            title="Close (Esc). An export under way carries on."
            @click="close"
          >
            Done
          </button>
        </header>

        <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
          <p class="text-xs text-text-muted">
            Makes a stick that CDJs and other players read: the playlists ticked, with their tracks’ cues, grids and
            waveforms. Exporting again brings it up to date, copying only what changed.
          </p>

          <!-- The stick -->
          <section class="flex flex-col gap-2">
            <div class="flex items-center justify-between">
              <h2 class="section-label">Stick</h2>
              <button
                class="rounded px-2 py-0.5 text-xs text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary disabled:opacity-50"
                :disabled="looking || !!exporting"
                @click="usb.load()"
              >
                {{ looking ? 'Looking…' : 'Look again' }}
              </button>
            </div>
            <p v-if="!devices.length" class="rounded border border-border bg-surface-1 px-3 py-4 text-center text-xs text-text-muted">
              {{ looking ? 'Looking for sticks…' : 'Plug in a USB stick or SD card. For most players it should be formatted as FAT32.' }}
            </p>
            <label
              v-for="d in devices"
              :key="d.path"
              class="flex cursor-pointer items-center gap-3 rounded border bg-surface-1 px-3 py-2"
              :class="d.path === path ? 'border-accent' : 'border-border'"
            >
              <input v-model="path" type="radio" :value="d.path" :disabled="!!exporting" class="size-3.5 accent-accent" />
              <span class="flex min-w-0 flex-1 flex-col">
                <span class="truncate text-sm">{{ d.name }}</span>
                <span class="truncate text-xs text-text-muted">{{ describe(d) }}</span>
              </span>
              <button
                class="shrink-0 rounded px-2 py-0.5 text-xs text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary disabled:opacity-50"
                title="Eject, so it’s safe to pull out"
                :disabled="exporting === d.path"
                @click.prevent="usb.eject(d.path)"
              >
                Eject
              </button>
            </label>
            <p v-if="fileSystemNote" class="text-xs text-status-busy">{{ fileSystemNote }}</p>
          </section>

          <!-- The playlists -->
          <section class="flex min-h-40 flex-1 flex-col gap-2">
            <div class="flex items-center justify-between">
              <h2 class="section-label">Playlists</h2>
              <button
                v-if="picked.size"
                class="rounded px-2 py-0.5 text-xs text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary disabled:opacity-50"
                :disabled="!!exporting"
                @click="picked = new Set()"
              >
                Untick all
              </button>
            </div>
            <div class="min-h-0 flex-1 overflow-y-auto rounded border border-border bg-surface-1 py-1">
              <p v-if="!playlists.length" class="px-3 py-4 text-center text-xs text-text-muted">
                The library has no playlists. Make one, and put on it what the stick should hold.
              </p>
              <label
                v-for="node in playlists"
                :key="node.id"
                class="flex cursor-pointer items-center gap-2 px-3 py-1 text-xs hover:bg-surface-2"
                :style="{ paddingLeft: `${0.75 + node.depth}rem` }"
              >
                <input
                  type="checkbox"
                  class="size-3.5 accent-accent"
                  :checked="ticked(node) === true"
                  :indeterminate="ticked(node) === 'some'"
                  :disabled="!!exporting || (node.kind === 'folder' && !leavesUnder(node).length)"
                  @change="toggle(node)"
                />
                <span class="min-w-0 flex-1 truncate" :class="node.kind === 'folder' ? 'font-medium' : ''">
                  {{ node.name }}
                </span>
                <span v-if="node.count !== null" class="shrink-0 text-text-muted tabular-nums">{{ node.count }}</span>
                <span v-else-if="node.kind === 'smart'" class="shrink-0 text-text-muted">intelligent</span>
              </label>
            </div>
            <p class="text-xs text-text-muted">
              The stick will hold exactly these: playlists exported before and no longer ticked come off it. Ones made
              on the stick itself, on a player, stay.
            </p>
          </section>

          <!-- How -->
          <section class="flex flex-wrap items-center gap-x-6 gap-y-2 text-xs">
            <label class="flex items-center gap-2" title="FLAC, ALAC and the like play on current players, not on older ones. Converted copies go on the stick; your files aren’t touched.">
              Files older players can’t read
              <select
                v-model="convert"
                :disabled="!!exporting"
                class="rounded border border-border bg-surface-1 px-2 py-1 text-xs outline-none focus:border-accent"
              >
                <option :value="null">Copy as they are</option>
                <option value="aiff">Convert to AIFF</option>
                <option value="wav">Convert to WAV</option>
                <option value="mp3">Convert to MP3 (320k)</option>
              </select>
            </label>
            <label class="flex cursor-pointer items-center gap-2">
              <input v-model="ejectAfter" type="checkbox" class="size-3.5 accent-accent" :disabled="!!exporting" />
              Eject when done
            </label>
          </section>
        </div>

        <!-- What it would do, how it's going, how it went -->
        <footer class="flex shrink-0 flex-col gap-2 border-t border-border bg-surface-1 px-4 py-3">
          <template v-if="exporting">
            <div class="flex items-center justify-between gap-3 text-xs">
              <span class="min-w-0 truncate">
                {{ going }}
                <span v-if="progress?.title" class="text-text-muted">· {{ progress.title }}</span>
              </span>
              <button
                class="shrink-0 rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
                title="Stop. The stick is left as it was before."
                :disabled="stopping || progress?.canCancel === false"
                @click="usb.cancel()"
              >
                Stop
              </button>
            </div>
            <ProgressBar :percent="percent ?? 100" :class="percent === null ? 'animate-pulse' : ''" />
            <p class="text-xs text-text-muted">Leave the stick in until this is done.</p>
          </template>
          <template v-else>
            <p v-if="error" class="error-box">{{ error }}</p>
            <div v-else-if="report" class="flex flex-col gap-1 text-xs">
              <p class="text-status-ok">
                {{ outcome }}
                {{ report.ejected ? 'It’s ejected, and safe to pull out.' : '' }}
              </p>
              <p v-if="report.ejectError" class="text-status-busy">It couldn’t be ejected: {{ report.ejectError }}</p>
              <details v-if="report.leftOut.length" class="text-status-busy">
                <summary class="cursor-pointer">
                  {{ report.leftOut.length }} {{ report.leftOut.length === 1 ? 'track has' : 'tracks have' }} something to look at
                </summary>
                <ul class="mt-1 max-h-24 overflow-y-auto text-text-secondary select-text">
                  <li v-for="line in report.leftOut" :key="line" class="truncate">{{ line }}</li>
                </ul>
              </details>
            </div>
            <div class="flex items-center justify-between gap-3">
              <p class="min-w-0 text-xs" :class="tooBig || estimateError ? 'text-status-error' : 'text-text-secondary'">
                <template v-if="!path">Choose a stick.</template>
                <template v-else-if="!chosen.length">Tick the playlists to put on it.</template>
                <template v-else-if="estimateError">{{ estimateError }}</template>
                <template v-else-if="tooBig && estimate">
                  Not enough room: this needs {{ formatBytes(estimate.neededBytes) }}, and the stick has
                  {{ formatBytes(estimate.freeBytes ?? 0) }} free.
                </template>
                <template v-else-if="estimate">{{ summary }}</template>
                <template v-else-if="estimating">Working out what to copy…</template>
              </p>
              <button
                class="shrink-0 rounded bg-accent px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
                :disabled="!ready"
                @click="start"
              >
                Export to {{ device?.name ?? 'USB' }}
              </button>
            </div>
          </template>
        </footer>
      </div>
    </div>
  </Teleport>
</template>
