<script setup lang="ts">
import { ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import ConsolidateLibrary from '@/components/ConsolidateLibrary.vue';
import LibraryFolders from '@/components/LibraryFolders.vue';
import SettingsSection from '@/components/SettingsSection.vue';
import ToggleRow from '@/components/ToggleRow.vue';
import { useLibrariesStore } from '@/stores/libraries';
import { useSettingsStore } from '@/stores/settings';
import { useSettingsSave } from '@/composables/useSettingsSave';
import { PALETTES, chosenPalette } from '@/composables/useWaveform';
import type { KeyNotation, PathTemplates } from '@/types/settings';

const settingsStore = useSettingsStore();
const { settings } = storeToRefs(settingsStore);
const { error, save } = useSettingsSave();

// The rekordbox library being managed.
const { status: library } = storeToRefs(useLibrariesStore());

// Beets' `paths` setting: which template files a track depends on its album.
const PATH_KINDS: { key: keyof PathTemplates; label: string }[] = [
  { key: 'default', label: 'Albums' },
  { key: 'comp', label: 'Compilations (Various Artists)' },
  { key: 'singleton', label: 'Tracks without an album' },
];
const paths = ref<PathTemplates>({ default: '', comp: '', singleton: '' });
// By what they say: saving any setting brings a new copy of them all, which shouldn't undo one being typed.
watch(
  () => JSON.stringify(settings.value?.paths),
  () => settings.value && (paths.value = { ...settings.value.paths }),
  { immediate: true },
);

const KEY_NOTATIONS: { id: KeyNotation; label: string; example: string }[] = [
  { id: 'standard', label: 'Musical', example: 'Am, F#' },
  { id: 'camelot', label: 'Camelot', example: '8A, 2B' },
];

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
    <div v-if="library?.masterDb" class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">rekordbox library</span>
      <code
        class="truncate rounded border border-border bg-surface-1 px-3 py-2 font-mono text-[11px] text-text-muted select-text"
        :title="library.masterDb"
      >
        {{ library.masterDb }}
      </code>
      <span class="text-xs text-text-muted">
        The library rekordbox keeps on this computer. The folders, paths and analysis below belong to it.
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
        label="Follow the tempo where it moves"
        description="Two beat detectors analyse every track. Where both find the tempo moving (a drummer who drifts, a DJ edit that changes pace) the beat grid follows it, as rekordbox's dynamic analysis does. Off, every grid is at one tempo, and the Grid column still says which tracks move."
        :model-value="settings.dynamicGrids"
        @update:model-value="(on) => save({ dynamicGrids: on })"
      />
    </div>

    <div class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">Keys</span>
      <div class="flex items-start justify-between gap-4 rounded border border-border bg-surface-1 px-3 py-2">
        <span class="flex flex-col">
          <span class="text-sm">Key notation</span>
          <span class="text-xs text-text-muted">
            How keys are shown and sorted here, whichever way each track's key is stored. Nothing in your rekordbox library is changed.
          </span>
        </span>
        <div class="flex shrink-0 overflow-hidden rounded border border-border">
          <button
            v-for="option in KEY_NOTATIONS"
            :key="option.id"
            class="px-3 py-1 text-xs transition-colors"
            :class="settings.keyNotation === option.id
              ? 'bg-surface-3 font-medium text-text-primary'
              : 'bg-surface-1 text-text-muted hover:text-text-secondary'"
            :title="`e.g. ${option.example}`"
            :aria-pressed="settings.keyNotation === option.id"
            @click="settings.keyNotation !== option.id && save({ keyNotation: option.id })"
          >
            {{ option.label }}
          </button>
        </div>
      </div>
    </div>

    <div class="flex flex-col gap-2">
      <span class="text-xs text-text-secondary">Waveforms</span>
      <div class="flex items-start justify-between gap-4 rounded border border-border bg-surface-1 px-3 py-2">
        <span class="flex flex-col">
          <span class="text-sm">Waveform colours</span>
          <span class="text-xs text-text-muted">
            How waveforms are drawn in the player and the track list, as in rekordbox. A track analysed without the chosen style is drawn in the nearest it has.
          </span>
        </span>
        <div class="flex shrink-0 overflow-hidden rounded border border-border">
          <button
            v-for="option in PALETTES"
            :key="option.id"
            class="px-3 py-1 text-xs transition-colors"
            :class="chosenPalette === option.id
              ? 'bg-surface-3 font-medium text-text-primary'
              : 'bg-surface-1 text-text-muted hover:text-text-secondary'"
            :aria-pressed="chosenPalette === option.id"
            @click="chosenPalette = option.id"
          >
            {{ option.label }}
          </button>
        </div>
      </div>
    </div>
  </SettingsSection>
</template>
