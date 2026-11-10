<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { open } from '@tauri-apps/plugin-dialog';
import { useLibraryStore } from '@/stores/library';
import { useSettingsStore } from '@/stores/settings';
import type { RelocateReport } from '@/types/library';

/**
 * The folders the library is kept in: the primary one (starred), where new
 * tracks go, and any others. Folders rekordbox has tracks in are offered.
 */
const libraryStore = useLibraryStore();
const settingsStore = useSettingsStore();
const { settings } = storeToRefs(settingsStore);
const { plan } = storeToRefs(libraryStore);

const moving = ref(false);
const report = ref<RelocateReport | null>(null);
const error = ref<string | null>(null);

const folders = computed(() => (settings.value ? [settings.value.musicDir, ...settings.value.libraryDirs] : []));
const tracksIn = (dir: string) => plan.value?.places.find((place) => place.path === dir)?.tracks;
/** Places rekordbox has tracks in that aren't library folders. */
const suggestions = computed(() => plan.value?.places.filter((place) => !place.library && place.tracks) ?? []);

const tracks = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'track' : 'tracks'}`;

async function run(action: () => Promise<unknown>) {
  error.value = null;
  report.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

async function addFolder() {
  const dir = await open({ directory: true });
  if (typeof dir === 'string') await run(() => settingsStore.addLibraryDir(dir));
}

/** Moves everything in the primary folder to a new one, which takes its place. */
async function movePrimary() {
  const dir = await open({ directory: true, defaultPath: settings.value?.musicDir });
  if (typeof dir !== 'string' || dir === settings.value?.musicDir) return;
  moving.value = true;
  await run(async () => {
    report.value = await libraryStore.changeMusicDir(dir);
    await settingsStore.load();
  });
  moving.value = false;
}

const buttonClass = 'shrink-0 text-xs text-text-muted transition-colors disabled:opacity-50';
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex items-center justify-between">
      <span class="text-xs text-text-secondary">Library folders</span>
      <button class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3" @click="addFolder">
        Add folder…
      </button>
    </div>
    <ul class="divide-y divide-border rounded border border-border">
      <li v-for="(dir, index) in folders" :key="dir" class="flex items-center gap-2 px-3 py-1.5">
        <button
          class="shrink-0 text-sm leading-none transition-colors"
          :class="index === 0 ? 'text-accent' : 'text-text-muted hover:text-text-secondary'"
          :disabled="index === 0"
          :title="index === 0 ? 'The primary folder: new tracks go here' : 'Make this the primary folder, where new tracks go'"
          :aria-label="index === 0 ? 'Primary folder' : 'Make primary'"
          @click="run(() => settingsStore.setPrimaryLibraryDir(dir))"
        >
          {{ index === 0 ? '★' : '☆' }}
        </button>
        <code class="min-w-0 flex-1 truncate font-mono text-xs text-text-secondary select-text" :title="dir">{{ dir }}</code>
        <span v-if="tracksIn(dir) !== undefined" class="shrink-0 text-xs text-text-muted">{{ tracks(tracksIn(dir)!) }}</span>
        <button
          v-if="index === 0"
          :disabled="moving"
          :class="[buttonClass, 'hover:text-text-secondary']"
          title="Move everything in this folder to another, which takes its place"
          @click="movePrimary"
        >
          {{ moving ? 'Moving…' : 'Move…' }}
        </button>
        <button
          v-else
          :class="[buttonClass, 'hover:text-status-error']"
          title="Stop treating this as a library folder. Its files and tracks stay."
          @click="run(() => settingsStore.removeLibraryDir(dir))"
        >
          Remove
        </button>
      </li>
    </ul>
    <span class="text-xs text-text-muted">
      New tracks go to the starred folder. Files in every library folder are tidied where they are,
      and shared on Soulseek when sharing is on.
    </span>

    <template v-if="suggestions.length">
      <span class="mt-1 text-xs text-text-secondary">Also in your rekordbox collection</span>
      <ul class="divide-y divide-border rounded border border-dashed border-border">
        <li v-for="place in suggestions" :key="place.path" class="flex items-center gap-2 px-3 py-1.5">
          <code class="min-w-0 flex-1 truncate font-mono text-xs text-text-secondary select-text" :title="place.path">
            {{ place.path }}
          </code>
          <span class="shrink-0 text-xs text-text-muted">{{ tracks(place.tracks) }}</span>
          <button
            :class="[buttonClass, 'hover:text-text-secondary']"
            title="Make this a library folder"
            @click="run(() => settingsStore.addLibraryDir(place.path))"
          >
            Add
          </button>
        </li>
      </ul>
    </template>

    <p v-if="report" class="text-xs text-status-ok">
      Moved {{ report.moved }} {{ report.moved === 1 ? 'file' : 'files' }}.
    </p>
    <div v-if="report?.failed.length" class="error-box">
      {{ report.failed.length }} couldn't be moved and are still in the old folder:
      <ul class="mt-1 list-disc pl-4 font-mono">
        <li v-for="f in report.failed" :key="f" class="break-all">{{ f }}</li>
      </ul>
    </div>
    <p v-if="error" class="error-box">{{ error }}</p>
  </div>
</template>
