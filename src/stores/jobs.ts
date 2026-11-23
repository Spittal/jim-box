import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Job, JobStatus } from '@/types/job';
import { trackKey, type TrackRequest } from '@/types/track';
import { showToast } from '@/composables/useToast';
import { useSettingsStore } from './settings';
import { useUiStore } from './ui';

const ACTIVE: JobStatus[] = ['queued', 'identifying', 'searching', 'waiting', 'downloading', 'tagging'];

export function isActive(job: Job): boolean {
  return ACTIVE.includes(job.status);
}

/** Stopped short of a file: something to look at. */
export function needsAttention(job: Job): boolean {
  return job.status === 'notFound' || job.status === 'failed';
}

/** Still wanted: on its way, or stopped short of a file. */
function isPending(job: Job): boolean {
  return isActive(job) || needsAttention(job);
}

/**
 * Downloads started within this long of each other are one batch, which
 * opens the Downloads window once: closed during a playlist's worth, it
 * stays closed.
 */
const BATCH_MS = 10_000;

export const useJobsStore = defineStore('jobs', () => {
  const jobs = ref<Job[]>([]);

  const activeCount = computed(() => jobs.value.filter(isActive).length);
  /** Fetches that haven't landed, newest first. */
  const pending = computed(() => jobs.value.filter(isPending));
  const notFoundCount = computed(() => jobs.value.filter((j) => j.status === 'notFound').length);
  /** Downloads that found no source or failed. */
  const attentionCount = computed(() => jobs.value.filter(needsAttention).length);
  /** Downloads that are over: done, or given up on. */
  const finished = computed(() => jobs.value.filter((j) => j.status === 'completed' || j.status === 'cancelled'));

  /** The fetch on its way to replace each file, by the file's path. */
  const replacing = computed(() => {
    const map = new Map<string, Job>();
    for (const job of jobs.value) {
      if (job.replaces && isActive(job) && !map.has(job.replaces)) map.set(job.replaces, job);
    }
    return map;
  });

  /** Latest job per track (see `trackKey`), so search results can show their status. */
  const jobByTrack = computed(() => {
    const map = new Map<string, Job>();
    for (const job of jobs.value) {
      const key = trackKey(job.request);
      if (!map.has(key)) map.set(key, job);
    }
    return map;
  });

  const jobFor = (track: TrackRequest): Job | undefined => jobByTrack.value.get(trackKey(track));

  function byId(id: string): Job | undefined {
    return jobs.value.find((j) => j.id === id);
  }

  /** Whether the jobs there were at launch are in: ones that resume then weren't just started. */
  let loaded = false;
  /** When a download last started, for telling a batch's first from the rest. */
  let lastStart = 0;

  async function load() {
    jobs.value = await invoke<Job[]>('list_jobs');
    loaded = true;
  }

  /** A download was just started, or started again: the Downloads window opens on the first of a batch. */
  function started(job: Job) {
    const now = Date.now();
    const first = now - lastStart > BATCH_MS;
    lastStart = now;
    const ui = useUiStore();
    if (!first || ui.downloads || ui.settings || ui.usb) return;
    if (useSettingsStore().settings?.openDownloadsOnFetch === false) return;
    ui.openDownloads(job.id);
  }

  /** Applies a `job-updated` event. */
  function upsert(job: Job) {
    const idx = jobs.value.findIndex((j) => j.id === job.id);
    const before = idx === -1 ? null : jobs.value[idx];
    if (idx === -1) jobs.value.unshift(job);
    else jobs.value[idx] = job;

    if (!loaded) return;
    if (isActive(job) && (!before || !isActive(before))) started(job);
    // A download landing while the window that shows it is closed is said.
    if (job.status === 'completed' && before && isActive(before) && !useUiStore().downloads) {
      const title = job.metadata?.title ?? job.request.title;
      showToast(job.alreadyInLibrary ? `“${title}” is already in your library` : `Downloaded “${title}”`);
    }
  }

  /** Applies a `job-progress` event: how far a transfer has got, and nothing else of its job. */
  function progressed(id: string, progress: Job['progress']) {
    const job = byId(id);
    if (job) job.progress = progress;
  }

  /** Applies a `job-removed` event. */
  function drop(id: string) {
    jobs.value = jobs.value.filter((j) => j.id !== id);
  }

  /**
   * Starts a download. `replaces` is a file to trash once it succeeds;
   * `playlistId` is a rekordbox playlist the track then joins.
   */
  async function start(track: TrackRequest, replaces?: string, playlistId?: string | null): Promise<Job> {
    const job = await invoke<Job>('start_job', {
      track,
      replaces: replaces ?? null,
      playlistId: playlistId ?? null,
    });
    upsert(job);
    return job;
  }

  /**
   * Fetches a track into the collection, and a playlist if one is given. A
   * fetch already on its way is left to it; one that stopped short starts again.
   */
  async function fetch(track: TrackRequest, playlistId?: string | null): Promise<Job> {
    const existing = jobFor(track);
    if (existing && isActive(existing)) return existing;
    if (existing && existing.status !== 'completed') await remove(existing.id);
    return start(track, undefined, playlistId);
  }

  /** Forgets the downloads that are over. Their files stay. */
  async function clearFinished() {
    for (const job of finished.value) await remove(job.id);
  }

  const retry = (id: string) => invoke('retry_job', { id });
  const retryNotFound = () => invoke('retry_not_found_jobs');
  /** Downloads a finished job again, replacing its file. */
  const redownload = (id: string) => invoke('redownload_job', { id });
  const cancel = (id: string) => invoke('cancel_job', { id });
  const remove = (id: string) => invoke('remove_job', { id });
  /** Gives up on a fetch: stops it if it's running, and forgets it. */
  async function discard(id: string) {
    const job = byId(id);
    if (job && isActive(job)) await cancel(id);
    await remove(id);
  }
  const downloadCandidate = (id: string, index: number) =>
    invoke('download_candidate', { id, index });
  /** Downloads the track from YouTube Music instead, as lossy AAC. */
  const downloadFromYoutube = (id: string) => invoke('download_from_youtube', { id });

  return {
    jobs,
    activeCount,
    pending,
    notFoundCount,
    attentionCount,
    finished,
    replacing,
    jobFor,
    byId,
    load,
    upsert,
    progressed,
    drop,
    start,
    fetch,
    retry,
    retryNotFound,
    redownload,
    cancel,
    remove,
    discard,
    clearFinished,
    downloadCandidate,
    downloadFromYoutube,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useJobsStore, import.meta.hot));
