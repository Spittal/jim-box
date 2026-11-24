<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import CoverArt from '@/components/CoverArt.vue';
import JobDetail from '@/components/JobDetail.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import { progressPercent } from '@/composables/useJobStatus';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { isActive, needsAttention, useJobsStore } from '@/stores/jobs';
import { useUiStore } from '@/stores/ui';
import { commandKey, isMac } from '@/platform';
import type { Job } from '@/types/job';

/**
 * Every download: the ones under way, the ones that stopped short, and the
 * finished ones. One picked shows its sources and what can be done about
 * it; several picked can be searched again or removed together.
 */
const ui = useUiStore();
const jobs = useJobsStore();
const collection = useCollectionStore();
const browse = useBrowseStore();

const sections = computed(() =>
  [
    { id: 'active', title: 'Under way', jobs: jobs.jobs.filter(isActive) },
    { id: 'attention', title: 'No source found, or failed', jobs: jobs.jobs.filter(needsAttention) },
    { id: 'finished', title: 'Finished', jobs: jobs.finished },
  ].filter((s) => s.jobs.length),
);
/** The rows in the order shown, for ranges and arrow keys. */
const ordered = computed(() => sections.value.flatMap((s) => s.jobs));

// --- What's picked --------------------------------------------------------------

const picked = ref(new Set<string>());
/** Where a shift-click range starts from. */
const anchor = ref<string | null>(null);

const pickedJobs = computed(() => ordered.value.filter((j) => picked.value.has(j.id)));
const shown = computed(() => (pickedJobs.value.length === 1 ? pickedJobs.value[0] : null));

function pickOnly(id: string) {
  picked.value = new Set([id]);
  anchor.value = id;
  ui.downloadFocus = id;
}

function click(job: Job, event: MouseEvent) {
  if (event.shiftKey && anchor.value) {
    const ids = ordered.value.map((j) => j.id);
    const [from, to] = [ids.indexOf(anchor.value), ids.indexOf(job.id)].sort((a, b) => a - b);
    if (from >= 0) {
      picked.value = new Set(ids.slice(from, to + 1));
      return;
    }
  }
  if (commandKey(event)) {
    const next = new Set(picked.value);
    if (next.has(job.id)) next.delete(job.id);
    else next.add(job.id);
    picked.value = next;
    anchor.value = job.id;
    return;
  }
  pickOnly(job.id);
}

/** Opens on the download asked for, or else the busiest one. */
function pickInitial() {
  const wanted = ui.downloadFocus && jobs.byId(ui.downloadFocus) ? ui.downloadFocus : ordered.value[0]?.id;
  if (wanted) pickOnly(wanted);
}

// Asked to show another download while open (a result's status clicked, say).
watch(
  () => ui.downloadFocus,
  (id) => {
    if (id && !(picked.value.size === 1 && picked.value.has(id)) && jobs.byId(id)) pickOnly(id);
  },
);

// Downloads removed (here, or by finishing a batch) are no longer picked.
watch(ordered, (now) => {
  const ids = new Set(now.map((j) => j.id));
  const left = [...picked.value].filter((id) => ids.has(id));
  if (left.length !== picked.value.size) {
    picked.value = new Set(left);
    if (!left.length) pickInitial();
  }
});

const list = ref<HTMLElement | null>(null);

async function scrollToPicked() {
  await nextTick();
  const id = anchor.value;
  if (id) list.value?.querySelector(`[data-job="${id}"]`)?.scrollIntoView({ block: 'nearest' });
}

/** ↑ and ↓ move through the list. */
function step(by: number) {
  const ids = ordered.value.map((j) => j.id);
  if (!ids.length) return;
  const at = anchor.value ? ids.indexOf(anchor.value) : -1;
  const next = ids[Math.min(ids.length - 1, Math.max(0, at + by))];
  pickOnly(next);
  void scrollToPicked();
}

// --- Doing things -----------------------------------------------------------------

const error = ref<string | null>(null);
const working = ref(false);

/** Does something to each of some downloads, carrying on past the ones that fail. */
async function each(targets: Job[], action: (job: Job) => Promise<unknown>) {
  working.value = true;
  error.value = null;
  try {
    for (const job of targets) {
      try {
        await action(job);
      } catch (e) {
        error.value = String(e);
      }
    }
  } finally {
    working.value = false;
  }
}

const pickedStopped = computed(() => pickedJobs.value.filter(needsAttention));
const pickedActive = computed(() => pickedJobs.value.filter(isActive));

const retryPicked = () => each(pickedStopped.value, (job) => jobs.retry(job.id));
const cancelPicked = () => each(pickedActive.value, (job) => jobs.cancel(job.id));
/** Gives up on the picked downloads: stops the ones running, and forgets them. Files already in the library stay. */
const removePicked = () => each(pickedJobs.value, (job) => jobs.discard(job.id));

function retryNotFound() {
  error.value = null;
  jobs.retryNotFound().catch((e) => (error.value = String(e)));
}

async function clearFinished() {
  working.value = true;
  error.value = null;
  try {
    await jobs.clearFinished();
  } catch (e) {
    error.value = String(e);
  } finally {
    working.value = false;
  }
}

/** The name of the playlist a download goes into, when it's one that's still there. */
function playlistName(job: Job): string | null {
  if (!job.playlistId) return null;
  return collection.playlists.find((p) => p.id === job.playlistId)?.name ?? null;
}

/** Shows a finished download's track in the list: in its playlist, when it went into one. */
async function showInLibrary(job: Job) {
  error.value = null;
  try {
    const title = job.metadata?.title ?? job.request.title;
    const artist = job.metadata?.artist ?? job.request.artist;
    const [id] = await invoke<(string | null)[]>('find_in_collection', { tracks: [{ title, artist }] });
    if (!id) {
      error.value = 'It isn’t in the collection yet. If rekordbox is open, it joins once you quit rekordbox.';
      return;
    }
    browse.select(playlistName(job) ? job.playlistId : null);
    await nextTick();
    browse.selectTracks([id]);
    ui.downloads = false;
  } catch (e) {
    error.value = String(e);
  }
}

// --- The window ---------------------------------------------------------------------

function close() {
  ui.downloads = false;
}

function onKey(event: KeyboardEvent) {
  if (event.defaultPrevented) return;
  const target = event.target as HTMLElement | null;
  if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    close();
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    step(event.key === 'ArrowDown' ? 1 : -1);
  } else if (event.key === 'a' && commandKey(event)) {
    event.preventDefault();
    picked.value = new Set(ordered.value.map((j) => j.id));
  }
}

onMounted(() => {
  pickInitial();
  void scrollToPicked();
  window.addEventListener('keydown', onKey);
});
onUnmounted(() => window.removeEventListener('keydown', onKey));

const buttonClass =
  'shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-6" @click.self="close">
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Downloads"
        class="flex h-[44rem] max-h-full w-full max-w-5xl flex-col overflow-hidden rounded-lg border border-border bg-surface-0 shadow-xl"
      >
        <header class="flex shrink-0 items-center gap-3 border-b border-border px-4 py-3">
          <h1 class="text-sm font-medium">Downloads</h1>
          <span class="min-w-0 flex-1 truncate text-xs text-text-muted">
            <template v-if="jobs.activeCount">{{ jobs.activeCount }} under way</template>
            <template v-if="jobs.activeCount && jobs.attentionCount"> · </template>
            <template v-if="jobs.attentionCount">{{ jobs.attentionCount }} to check</template>
          </span>
          <button
            v-if="jobs.notFoundCount"
            :class="buttonClass"
            title="Search Soulseek again for every download that found no source"
            @click="retryNotFound"
          >
            Search again for {{ jobs.notFoundCount }} not found
          </button>
          <button
            v-if="jobs.finished.length"
            :class="buttonClass"
            :disabled="working"
            title="Take the finished downloads off this list. Their tracks stay in your library."
            @click="clearFinished"
          >
            Clear finished
          </button>
          <button :class="buttonClass" title="Close (Esc). Downloads carry on." @click="close">Done</button>
        </header>

        <p v-if="error" class="error-box mx-4 mt-3">{{ error }}</p>

        <p v-if="!ordered.length" class="m-auto max-w-sm text-center text-sm text-text-muted">
          Nothing downloaded yet. Find tracks online with the search at the foot of the window
          ({{ isMac ? '⇧⌘F' : 'Ctrl+Shift+F' }}), or pick “Download” on a track.
        </p>

        <div v-else class="flex min-h-0 flex-1">
          <!-- Every download -->
          <ul ref="list" class="w-[24rem] shrink-0 overflow-y-auto border-r border-border select-none">
            <template v-for="section in sections" :key="section.id">
              <li
                class="sticky top-0 z-10 border-b border-border bg-surface-1 px-4 py-1 text-[10px] tracking-wide text-text-muted uppercase"
                :class="section.id === 'attention' ? 'text-status-error' : ''"
              >
                {{ section.title }} ({{ section.jobs.length }})
              </li>
              <li
                v-for="job in section.jobs"
                :key="job.id"
                :data-job="job.id"
                class="flex cursor-default items-center gap-3 border-b border-border/50 px-4 py-2"
                :class="picked.has(job.id) ? 'bg-surface-2' : 'hover:bg-surface-1'"
                @click="click(job, $event)"
              >
                <CoverArt :src="job.request.coverUrl" class="size-9 shrink-0" />
                <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span class="truncate text-xs text-text-primary">{{ job.metadata?.title ?? job.request.title }}</span>
                  <span class="truncate text-[11px] text-text-muted">
                    {{ job.metadata?.artist ?? job.request.artist }}
                    <template v-if="job.replaces"> · replacing a file</template>
                    <template v-else-if="playlistName(job)"> · into {{ playlistName(job) }}</template>
                  </span>
                  <ProgressBar v-if="progressPercent(job) !== null" :percent="progressPercent(job)!" class="mt-0.5" />
                  <JobStatusBadge
                    v-else
                    :job="job"
                    class="text-[11px] text-text-secondary"
                    :title="job.error ?? undefined"
                  />
                </div>
              </li>
            </template>
          </ul>

          <!-- What's picked -->
          <div class="min-w-0 flex-1 overflow-y-auto">
            <template v-if="shown">
              <div v-if="shown.status === 'completed'" class="flex justify-end px-5 pt-4">
                <button :class="buttonClass" title="Close this and select the track in the list" @click="showInLibrary(shown)">
                  Show in library
                </button>
              </div>
              <JobDetail :key="shown.id" :job="shown" class="pt-4" />
            </template>
            <div v-else-if="pickedJobs.length" class="flex flex-col items-start gap-3 p-5">
              <p class="text-sm">{{ pickedJobs.length }} downloads selected</p>
              <div class="flex flex-wrap gap-2">
                <button
                  v-if="pickedStopped.length"
                  class="rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
                  :disabled="working"
                  title="Search again for the selected downloads that found no source or failed"
                  @click="retryPicked"
                >
                  Search again for {{ pickedStopped.length }}
                </button>
                <button
                  v-if="pickedActive.length"
                  :class="buttonClass"
                  :disabled="working"
                  title="Stop the selected downloads that are under way"
                  @click="cancelPicked"
                >
                  Cancel {{ pickedActive.length }}
                </button>
                <button
                  :class="buttonClass"
                  :disabled="working"
                  title="Stop the selected downloads that are running, and take them all off this list. Tracks already in your library stay."
                  @click="removePicked"
                >
                  Remove {{ pickedJobs.length }}
                </button>
              </div>
            </div>
            <p v-else class="p-5 text-sm text-text-muted">Pick a download to see its sources.</p>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>
