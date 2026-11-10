import { reactive, readonly } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { trackKey, type TrackRequest } from '@/types/track';

/**
 * Deezer's 30-second clip of a search result, one at a time. MusicBrainz has
 * no audio, so its results have nothing to play.
 */
const state = reactive({
  /** The result whose clip is loading or playing, as its `trackKey`. */
  key: null as string | null,
  loading: false,
  /** How far through the clip, from 0 to 1. */
  progress: 0,
  /** The result whose clip couldn't be played. */
  failedKey: null as string | null,
});

let audio: HTMLAudioElement | null = null;
/** Counts plays and stops, so a late answer to an earlier one is dropped. */
let turn = 0;

function element(): HTMLAudioElement {
  if (audio) return audio;
  audio = new Audio();
  audio.addEventListener('timeupdate', () => {
    if (audio!.duration) state.progress = audio!.currentTime / audio!.duration;
  });
  audio.addEventListener('ended', stopPreview);
  return audio;
}

async function start(url: string) {
  const el = element();
  el.src = url;
  await el.play();
}

export function canPreview(track: TrackRequest): boolean {
  return track.deezerId !== null && (!!track.previewUrl || track.previewUrl === undefined);
}

export function stopPreview() {
  turn++;
  audio?.pause();
  state.key = null;
  state.loading = false;
  state.progress = 0;
}

/** Plays the result's clip, or stops it if it's the one playing. */
export async function togglePreview(track: TrackRequest) {
  const key = trackKey(track);
  if (state.key === key) return stopPreview();
  stopPreview();
  if (track.deezerId === null) return;
  const mine = turn;
  state.key = key;
  state.loading = true;
  state.failedKey = null;
  try {
    try {
      if (!track.previewUrl) throw new Error('no link');
      await start(track.previewUrl);
    } catch (e) {
      if (mine !== turn) return;
      // The link that came with the results has run out: ask for a new one.
      const fresh = await invoke<string | null>('track_preview', { deezerId: track.deezerId });
      if (mine !== turn) return;
      if (!fresh) throw e;
      await start(fresh);
    }
    if (mine === turn) state.loading = false;
  } catch {
    if (mine !== turn) return;
    stopPreview();
    state.failedKey = key;
  }
}

export const preview = readonly(state);
