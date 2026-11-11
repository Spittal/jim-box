<script setup lang="ts">
import { ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import ConsolidateLibrary from '@/components/ConsolidateLibrary.vue';
import LibraryChooser from '@/components/LibraryChooser.vue';
import LibraryFolders from '@/components/LibraryFolders.vue';
import SettingsSection from '@/components/SettingsSection.vue';
import ToggleRow from '@/components/ToggleRow.vue';
import { useLibrariesStore } from '@/stores/libraries';
import { useSettingsStore } from '@/stores/settings';
import { useSettingsSave } from '@/composables/useSettingsSave';
import type { PathTemplates } from '@/types/settings';

const settingsStore = useSettingsStore();
const { settings } = storeToRefs(settingsStore);
const { error, save } = useSettingsSave();

// Which rekordbox library is being managed.
const librariesStore = useLibrariesStore();
const { current, opensByDefault } = storeToRefs(librariesStore);
const switching = ref(false);

async function showOthers() {
  switching.value = !switching.value;
  // Drives come and go.
  if (switching.value) await librariesStore.load();
}

// Beets' `paths` setting: which template files a track depends on its album.
const PATH_KINDS: { key: keyof PathTemplates; label: string }[] = [
  { key: 'default', label: 'Albums' },
  { key: 'comp', label: 'Compilations (Various Artists)' },
  { key: 'singleton', label: 'Tracks without an album' },
];
const paths = ref<PathTemplates>({ default: '', comp: '', singleton: '' });
watch(() => settings.value?.paths, (p) => p && (paths.value = { ...p }), { immediate: true });

const fields = ['albumartist', 'artist', 'album', 'year', 'track', 'disc', 'title'].map((f) => `$${f}`);

async function savePaths() {
  const current = settings.value?.paths;
  if (current && PATH_KINDS.some(({ key }) => paths.value[key] !== current[key])) {
    await save({ paths: { ...paths.value } });
  }
}
</script>

<template>
  <SettingsSection
    v-if="settings"
    title="Library"
    description="Finished downloads are tagged, moved into your primary library folder laid out the way beets would, and added to your rekordbox collection."
    :error="error"
  >
    <div v-if="current" class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">rekordbox library</span>
      <div class="flex items-center gap-3 rounded border border-border bg-surface-1 px-3 py-2">
        <div class="flex min-w-0 flex-1 flex-col">
          <span class="truncate text-sm">{{ current.name }}</span>
          <code class="truncate font-mono text-[11px] text-text-muted select-text" :title="current.masterDb">
            {{ current.masterDb }}
          </code>
        </div>
        <button
          class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
          @click="showOthers"
        >
          {{ switching ? 'Cancel' : 'Switch library…' }}
        </button>
      </div>
      <LibraryChooser v-if="switching" switching />
      <ToggleRow
        v-else
        label="Open this library by default"
        description="Off, jim-box asks which library to open each time it starts."
        :model-value="opensByDefault"
        @update:model-value="(on) => librariesStore.setDefault(on)"
      />
      <span class="text-xs text-text-muted">
        The folders, paths and analysis below, along with downloads, playlists, formats and what's
        shared, belong to this library. Your Soulseek and YouTube sign-ins are used for all of them.
      </span>
    </div>

    <LibraryFolders />

    <ConsolidateLibrary />

    <div class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">Paths</span>
      <label v-for="kind in PATH_KINDS" :key="kind.key" class="flex flex-col gap-1">
        <span class="text-[11px] text-text-muted">{{ kind.label }}</span>
        <input
          v-model="paths[kind.key]"
          type="text"
          spellcheck="false"
          class="rounded border border-border bg-surface-1 px-3 py-1.5 font-mono text-xs outline-none focus:border-accent"
          @blur="savePaths"
          @keydown.enter="savePaths"
        />
      </label>
      <span class="text-xs text-text-muted">
        Beets path syntax. Fields:
        <code v-for="f in fields" :key="f" class="mr-1 font-mono text-text-secondary">{{ f }}</code>
        · <code class="font-mono">/</code> makes folders; the file extension is added automatically.
      </span>
    </div>

    <div class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">Analysis</span>
      <ToggleRow
        label="Analyse new tracks"
        description="Work out the BPM, beat grid, key and waveforms of tracks as they join the collection, so they're ready for rekordbox and CDJs."
        :model-value="settings.analyseNewTracks"
        @update:model-value="(on) => save({ analyseNewTracks: on })"
      />
      <ToggleRow
        label="Follow tempo changes"
        description="Let a beat grid change tempo part-way through a track, for live drummers and DJ edits. Off, one tempo is fitted to the whole track, as rekordbox's normal analysis does."
        :model-value="settings.followTempoChanges"
        @update:model-value="(on) => save({ followTempoChanges: on })"
      />
    </div>
  </SettingsSection>
</template>
