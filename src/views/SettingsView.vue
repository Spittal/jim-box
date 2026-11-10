<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue';
import { storeToRefs } from 'pinia';
import SettingsSoulseek from '@/components/SettingsSoulseek.vue';
import SettingsLibrary from '@/components/SettingsLibrary.vue';
import SettingsSharing from '@/components/SettingsSharing.vue';
import SettingsFormats from '@/components/SettingsFormats.vue';
import SettingsYoutube from '@/components/SettingsYoutube.vue';
import { useUiStore, type SettingsTab } from '@/stores/ui';

/** Settings, shown over the collection. */
const sections: { id: SettingsTab; label: string }[] = [
  { id: 'library', label: 'Library' },
  { id: 'formats', label: 'Formats' },
  { id: 'soulseek', label: 'Soulseek' },
  { id: 'sharing', label: 'Sharing' },
  { id: 'youtube', label: 'YouTube Music' },
];

const { settings: active } = storeToRefs(useUiStore());

function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.defaultPrevented) active.value = null;
}
onMounted(() => window.addEventListener('keydown', onKey));
onUnmounted(() => window.removeEventListener('keydown', onKey));
</script>

<template>
  <div class="absolute inset-0 z-30 flex flex-col bg-surface-0">
    <header class="flex items-center justify-between border-b border-border px-4 py-3">
      <h1 class="text-sm font-medium">Settings</h1>
      <button
        class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
        title="Back to the collection (Esc)"
        @click="active = null"
      >
        Done
      </button>
    </header>
    <div class="flex min-h-0 flex-1">
      <nav class="w-40 shrink-0 border-r border-border py-2">
        <button
          v-for="section in sections"
          :key="section.id"
          class="block w-full px-4 py-1.5 text-left text-xs transition-colors"
          :class="active === section.id
            ? 'bg-surface-1 font-medium text-text-primary'
            : 'text-text-muted hover:text-text-secondary'"
          @click="active = section.id"
        >
          {{ section.label }}
        </button>
      </nav>

      <div class="flex-1 overflow-y-auto p-5">
        <SettingsLibrary v-if="active === 'library'" />
        <SettingsFormats v-if="active === 'formats'" />
        <SettingsSoulseek v-if="active === 'soulseek'" />
        <SettingsSharing v-if="active === 'sharing'" />
        <SettingsYoutube v-if="active === 'youtube'" />
      </div>
    </div>
  </div>
</template>
