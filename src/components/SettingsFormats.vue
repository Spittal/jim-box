<script setup lang="ts">
import { storeToRefs } from 'pinia';
import SettingsSection from '@/components/SettingsSection.vue';
import ToggleRow from '@/components/ToggleRow.vue';
import { useSettingsStore } from '@/stores/settings';
import { useSettingsSave } from '@/composables/useSettingsSave';
import type { LosslessFormat } from '@/types/settings';

const { settings } = storeToRefs(useSettingsStore());
const { error, save } = useSettingsSave();

const formats: { id: LosslessFormat; label: string; description: string }[] = [
  { id: 'flac', label: 'FLAC', description: 'The standard lossless format. Best tag support.' },
  { id: 'alac', label: 'ALAC', description: 'Apple Lossless (.m4a). Only files that report lossless properties are accepted.' },
  { id: 'aiff', label: 'AIFF', description: 'Uncompressed. Larger files; tags are supported.' },
  { id: 'wav', label: 'WAV', description: 'Uncompressed. Tag support varies between players.' },
];

function setFormat(id: LosslessFormat, enabled: boolean) {
  const current = settings.value?.formats ?? [];
  const next = enabled ? [...current, id] : current.filter((f) => f !== id);
  save({ formats: formats.map((f) => f.id).filter((f) => next.includes(f)) });
}
</script>

<template>
  <SettingsSection
    v-if="settings"
    title="Formats"
    description="Lossless files are always preferred. Choose which formats count."
    :error="error"
  >
    <div class="flex flex-col gap-2">
      <ToggleRow
        v-for="f in formats"
        :key="f.id"
        :label="f.label"
        :description="f.description"
        :model-value="settings.formats.includes(f.id)"
        @update:model-value="(on) => setFormat(f.id, on)"
      />
    </div>
    <ToggleRow
      label="Prefer hi-res"
      description="Rank 24-bit and high sample rate files above CD quality when both are available."
      :model-value="settings.preferHiRes"
      @update:model-value="(on) => save({ preferHiRes: on })"
    />
    <ToggleRow
      label="Fall back to lossy"
      description="When no lossless copy is found, download the best lossy one (MP3, AAC, Ogg or Opus, 192 kbps or better). Library scans can upgrade it later."
      :model-value="settings.lossyFallback"
      @update:model-value="(on) => save({ lossyFallback: on })"
    />
  </SettingsSection>
</template>
