import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Job, JobStatus } from '@/types/job';
import { trackKey, type TrackRequest } from '@/types/track';

const ACTIVE: JobStatus[] = ['queued', 'identifying', 'searching', 'waiting', 'downloading', 'tagging'];

export function isActive(job: Job): boolean {
  return ACTIVE.includes(job.status);
}

/** Still wanted: on its way, or stopped short of a file. Shown among the tracks. */
export function isPending(job: Job): boolean {
  return isActive(job) || job.status === 'notFound' || job.status === 'failed';
}

export const useJobsStore = defineStore('jobs', () => {
  const jobs = ref<Job[]>([]);

  const activeCount = computed(() => jobs.value.filter(isActive).length);
  /** Fetches that haven't landed, newest first. */
  const pending = computed(() => jobs.value.filter(isPending));
  const notFoundCount = computed(() => jobs.value.filter((j) => j.status === 'notFound').length);

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

  async function load() {
    jobs.value = await invoke<Job[]>('list_jobs');
  }

  /** Applies a `job-updated` event. */
  function upsert(job: Job) {
    const idx = jobs.value.findIndex((j) => j.id === job.id);
    if (idx === -1) jobs.value.unshift(job);
    else jobs.value[idx] = job;
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

  const retry = (id: string) => invoke('retry_job', { id });
  const retryNotFound = () => invoke('retry_not_found_jobs');
  /** Downloads a finished job again, replacing its file. */
  const redownload = (id: string) => invoke('redownload_job', { id });
  const cancel = (id: string) => invoke('cancel_job', { id });
  const remove = (id: string) => invoke('remove_job', { id });
  const downloadCandidate = (id: string, index: number) =>
    invoke('download_candidate', { id, index });
  /** Downloads the track from YouTube Music instead, as lossy AAC. */
  const downloadFromYoutube = (id: string) => invoke('download_from_youtube', { id });

  return {
    jobs,
    activeCount,
    pending,
    notFoundCount,
    replacing,
    jobByTrack,
    jobFor,
    byId,
    load,
    upsert,
    drop,
    start,
    fetch,
    retry,
    retryNotFound,
    redownload,
    cancel,
    remove,
    downloadCandidate,
    downloadFromYoutube,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useJobsStore, import.meta.hot));
