<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue';
import { storeToRefs } from 'pinia';
import SettingsSoulseek from '@/components/SettingsSoulseek.vue';
import SettingsLibrary from '@/components/SettingsLibrary.vue';
import SettingsSharing from '@/components/SettingsSharing.vue';
import SettingsFormats from '@/components/SettingsFormats.vue';
import SettingsYoutube from '@/components/SettingsYoutube.vue';
import SettingsShortcuts from '@/components/SettingsShortcuts.vue';
import { useUiStore, type SettingsTab } from '@/stores/ui';

/** Settings, in a dialog over the window. */
const sections: { id: SettingsTab; label: string }[] = [
  { id: 'library', label: 'Library' },
  { id: 'formats', label: 'Formats' },
  { id: 'soulseek', label: 'Soulseek' },
  { id: 'sharing', label: 'Sharing' },
  { id: 'youtube', label: 'YouTube Music' },
  { id: 'shortcuts', label: 'Shortcuts' },
];

const { settings: active } = storeToRefs(useUiStore());

function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.defaultPrevented) active.value = null;
}
onMounted(() => window.addEventListener('keydown', onKey));
onUnmounted(() => window.removeEventListener('keydown', onKey));
</script>

<template>
  <!-- Over the whole window, like the other dialogs. -->
  <Teleport to="body">
    <div
      class="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-6"
      @click.self="active = null"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        class="flex h-[36rem] max-h-full w-full max-w-3xl flex-col overflow-hidden rounded-lg border border-border bg-surface-0 shadow-xl"
      >
        <header class="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
          <h1 class="text-sm font-medium">Settings</h1>
          <button
            class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
            title="Close (Esc)"
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

          <div class="min-w-0 flex-1 overflow-y-auto p-5">
            <SettingsLibrary v-if="active === 'library'" />
            <SettingsFormats v-if="active === 'formats'" />
            <SettingsSoulseek v-if="active === 'soulseek'" />
            <SettingsSharing v-if="active === 'sharing'" />
            <SettingsYoutube v-if="active === 'youtube'" />
            <SettingsShortcuts v-if="active === 'shortcuts'" />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>
