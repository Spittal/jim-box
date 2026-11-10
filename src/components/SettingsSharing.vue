<script setup lang="ts">
import { storeToRefs } from 'pinia';
import { open } from '@tauri-apps/plugin-dialog';
import SettingsSection from '@/components/SettingsSection.vue';
import ToggleRow from '@/components/ToggleRow.vue';
import { useSettingsStore } from '@/stores/settings';
import { useSettingsSave } from '@/composables/useSettingsSave';

const { settings } = storeToRefs(useSettingsStore());
const { error, save } = useSettingsSave();

async function addFolder() {
  const dir = await open({ directory: true });
  if (typeof dir !== 'string' || !settings.value) return;
  if (settings.value.sharedDirs.includes(dir)) return;
  await save({ sharedDirs: [...settings.value.sharedDirs, dir] });
}

function removeFolder(dir: string) {
  if (!settings.value) return;
  save({ sharedDirs: settings.value.sharedDirs.filter((d) => d !== dir) });
}
</script>

<template>
  <SettingsSection
    v-if="settings"
    title="Sharing"
    description="Soulseek users expect you to share. Many peers won't upload to people who share nothing. Changes reconnect Soulseek."
    :error="error"
  >
    <ToggleRow
      label="Share my jim-box library"
      :description="[settings.musicDir, ...settings.libraryDirs].join(' · ')"
      :model-value="settings.shareMusicDir"
      @update:model-value="(on) => save({ shareMusicDir: on })"
    />

    <div class="flex flex-col gap-2">
      <div class="flex items-center justify-between">
        <span class="text-xs text-text-secondary">Other shared folders</span>
        <button
          class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
          @click="addFolder"
        >
          Add folder…
        </button>
      </div>
      <ul v-if="settings.sharedDirs.length" class="divide-y divide-border rounded border border-border">
        <li
          v-for="dir in settings.sharedDirs"
          :key="dir"
          class="flex items-center justify-between gap-2 px-3 py-1.5"
        >
          <code class="truncate font-mono text-xs text-text-secondary">{{ dir }}</code>
          <button
            class="shrink-0 text-xs text-text-muted transition-colors hover:text-status-error"
            @click="removeFolder(dir)"
          >
            Remove
          </button>
        </li>
      </ul>
      <p v-else class="text-xs text-text-muted">None.</p>
    </div>
  </SettingsSection>
</template>
