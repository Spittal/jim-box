import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Job, JobStatus } from '@/types/job';
import { trackKey, type TrackRequest } from '@/types/track';

const ACTIVE: JobStatus[] = ['queued', 'identifying', 'searching', 'waiting', 'downloading', 'tagging'];

export function isActive(job: Job): boolean {
  return ACTIVE.includes(job.status);
}

export const useJobsStore = defineStore('jobs', () => {
  const jobs = ref<Job[]>([]);

  const activeCount = computed(() => jobs.value.filter(isActive).length);

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
   * `playlist` names a rekordbox playlist the track then joins.
   */
  async function start(track: TrackRequest, replaces?: string, playlist?: string): Promise<Job> {
    const job = await invoke<Job>('start_job', {
      track,
      replaces: replaces ?? null,
      playlist: playlist ?? null,
    });
    upsert(job);
    return job;
  }

  const retry = (id: string) => invoke('retry_job', { id });
  const retryNotFound = () => invoke('retry_not_found_jobs');
  /** Downloads a finished job again, replacing its file. */
  const redownload = (id: string) => invoke('redownload_job', { id });
  const cancel = (id: string) => invoke('cancel_job', { id });
  const remove = (id: string) => invoke('remove_job', { id });
  const clearFinished = () => invoke('clear_finished_jobs');
  const downloadCandidate = (id: string, index: number) =>
    invoke('download_candidate', { id, index });
  /** Downloads the track from YouTube Music instead, as lossy AAC. */
  const downloadFromYoutube = (id: string) => invoke('download_from_youtube', { id });

  return {
    jobs,
    activeCount,
    jobByTrack,
    jobFor,
    byId,
    load,
    upsert,
    drop,
    start,
    retry,
    retryNotFound,
    redownload,
    cancel,
    remove,
    clearFinished,
    downloadCandidate,
    downloadFromYoutube,
  };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useJobsStore, import.meta.hot));
