<script setup lang="ts">
import { ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { open } from '@tauri-apps/plugin-dialog';
import ConsolidateLibrary from '@/components/ConsolidateLibrary.vue';
import SettingsSection from '@/components/SettingsSection.vue';
import ToggleRow from '@/components/ToggleRow.vue';
import { useSettingsStore } from '@/stores/settings';
import { useLibraryStore } from '@/stores/library';
import { useSettingsSave } from '@/composables/useSettingsSave';
import type { RelocateReport } from '@/types/library';
import type { PathTemplates } from '@/types/settings';

const settingsStore = useSettingsStore();
const libraryStore = useLibraryStore();
const { settings } = storeToRefs(settingsStore);
const { error, save } = useSettingsSave();

const moving = ref(false);
const report = ref<RelocateReport | null>(null);

// Beets' `paths` setting: which template files a track depends on its album.
const PATH_KINDS: { key: keyof PathTemplates; label: string }[] = [
  { key: 'default', label: 'Albums' },
  { key: 'comp', label: 'Compilations (Various Artists)' },
  { key: 'singleton', label: 'Tracks without an album' },
];
const paths = ref<PathTemplates>({ default: '', comp: '', singleton: '' });
watch(() => settings.value?.paths, (p) => p && (paths.value = { ...p }), { immediate: true });

const fields = ['albumartist', 'artist', 'album', 'year', 'track', 'disc', 'title'].map((f) => `$${f}`);

/** Switches folders, moving everything from the old one into the new one. */
async function chooseFolder() {
  const dir = await open({ directory: true, defaultPath: settings.value?.musicDir });
  if (typeof dir !== 'string' || dir === settings.value?.musicDir) return;
  moving.value = true;
  error.value = null;
  report.value = null;
  try {
    report.value = await libraryStore.changeMusicDir(dir);
    await settingsStore.load();
  } catch (e) {
    error.value = String(e);
  } finally {
    moving.value = false;
  }
}

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
    description="Finished downloads are tagged, moved into your music folder laid out the way beets would, and added to your rekordbox collection."
    :error="error"
  >
    <div class="flex flex-col gap-1">
      <span class="text-xs text-text-secondary">Music folder</span>
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate rounded border border-border bg-surface-1 px-3 py-1.5 font-mono text-xs select-text">
          {{ settings.musicDir }}
        </code>
        <button
          :disabled="moving"
          class="rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
          @click="chooseFolder"
        >
          {{ moving ? 'Moving…' : 'Choose…' }}
        </button>
      </div>
      <span class="text-xs text-text-muted">
        Changing it moves everything in the current folder to the new one, and points the tracks
        in your rekordbox collection at their new place. Nothing is overwritten.
      </span>
      <p v-if="report" class="text-xs text-status-ok">
        Moved {{ report.moved }} {{ report.moved === 1 ? 'file' : 'files' }}.
      </p>
      <div v-if="report?.failed.length" class="error-box">
        {{ report.failed.length }} couldn't be moved and are still in the old folder:
        <ul class="mt-1 list-disc pl-4 font-mono">
          <li v-for="f in report.failed" :key="f" class="break-all">{{ f }}</li>
        </ul>
      </div>
    </div>

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
