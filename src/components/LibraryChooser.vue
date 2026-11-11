<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { open as pickFile } from '@tauri-apps/plugin-dialog';
import { useLibrariesStore } from '@/stores/libraries';
import type { LibraryInfo } from '@/types/libraries';

/**
 * Picks the rekordbox library to manage: one opened before, one found on
 * this machine or a connected drive, or any `master.db`. Shown at launch,
 * and in settings (`switching`) to change library, which restarts the app.
 */
const props = defineProps<{ switching?: boolean }>();

const librariesStore = useLibrariesStore();
const { choices } = storeToRefs(librariesStore);

const libraries = computed(
  () => choices.value?.libraries.filter((library) => !props.switching || library.id !== choices.value?.open) ?? [],
);

// Ticked to begin with for a first library, as most people have the one.
const makeDefault = ref(
  choices.value ? choices.value.defaultId !== null || !choices.value.libraries.some((library) => library.id) : false,
);
/** The path being opened. */
const opening = ref<string | null>(null);
const restarting = ref(false);
/** The library whose Forget is waiting to be confirmed. */
const forgetting = ref<string | null>(null);
const error = ref<string | null>(null);

async function openLibrary(masterDb: string) {
  if (opening.value) return;
  error.value = null;
  opening.value = masterDb;
  try {
    restarting.value = (await librariesStore.open(masterDb, makeDefault.value)) === 'restarting';
  } catch (e) {
    error.value = String(e);
  }
  if (!restarting.value) opening.value = null;
}

async function choose() {
  const file = await pickFile({ filters: [{ name: 'rekordbox library (master.db)', extensions: ['db'] }] });
  if (typeof file === 'string') await openLibrary(file);
}

async function forget(library: LibraryInfo) {
  if (!library.id) return;
  error.value = null;
  forgetting.value = null;
  try {
    await librariesStore.forget(library.id);
  } catch (e) {
    error.value = String(e);
  }
}

const smallButton = 'shrink-0 text-xs text-text-muted transition-colors disabled:opacity-50';
</script>

<template>
  <div class="flex flex-col gap-3">
    <ul v-if="libraries.length" class="divide-y divide-border rounded border border-border">
      <li v-for="library in libraries" :key="library.masterDb" class="flex items-center gap-3 px-3 py-2">
        <div class="flex min-w-0 flex-1 flex-col">
          <span class="truncate text-sm">
            {{ library.name }}
            <span v-if="!library.available" class="text-xs text-text-muted">· not connected</span>
          </span>
          <code class="truncate font-mono text-[11px] text-text-muted select-text" :title="library.masterDb">
            {{ library.masterDb }}
          </code>
        </div>
        <template v-if="library.id && forgetting === library.id">
          <span class="shrink-0 text-xs text-text-muted">Forget its downloads and settings?</span>
          <button :class="[smallButton, 'hover:text-status-error']" @click="forget(library)">Forget</button>
          <button :class="[smallButton, 'hover:text-text-secondary']" @click="forgetting = null">Cancel</button>
        </template>
        <template v-else>
          <button
            v-if="library.id"
            :class="[smallButton, 'hover:text-status-error']"
            title="Take this library off the list, with the downloads, playlists and settings jim-box saved for it. The rekordbox library and its music aren't touched."
            @click="forgetting = library.id"
          >
            Forget
          </button>
          <button
            class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
            :disabled="!library.available || opening !== null"
            @click="openLibrary(library.masterDb)"
          >
            {{ opening === library.masterDb ? (restarting ? 'Restarting…' : 'Opening…') : 'Open' }}
          </button>
        </template>
      </li>
    </ul>

    <div class="flex items-center justify-between gap-4">
      <label class="flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
        <input v-model="makeDefault" type="checkbox" class="size-4 accent-accent" />
        Open this library by default
      </label>
      <button
        class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
        :disabled="opening !== null"
        title="Choose a rekordbox library's master.db file"
        @click="choose"
      >
        Choose a library…
      </button>
    </div>

    <p v-if="switching" class="text-xs text-text-muted">
      jim-box restarts to switch library. Downloads in progress stop, and can be retried when you
      come back to this one.
    </p>
    <p v-if="error" class="error-box">{{ error }}</p>
  </div>
</template>
