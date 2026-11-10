<script setup lang="ts">
import { computed } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import SettingsSoulseek from '@/components/SettingsSoulseek.vue';
import SettingsLibrary from '@/components/SettingsLibrary.vue';
import SettingsSharing from '@/components/SettingsSharing.vue';
import SettingsFormats from '@/components/SettingsFormats.vue';
import SettingsYoutube from '@/components/SettingsYoutube.vue';

type Section = 'soulseek' | 'library' | 'sharing' | 'formats' | 'youtube';

const sections: { id: Section; label: string }[] = [
  { id: 'library', label: 'Library' },
  { id: 'formats', label: 'Formats' },
  { id: 'soulseek', label: 'Soulseek' },
  { id: 'sharing', label: 'Sharing' },
  { id: 'youtube', label: 'YouTube Music' },
];

const route = useRoute();
const router = useRouter();

// The URL is the source of truth for the active tab.
const active = computed<Section>(() => {
  const tab = route.query.tab;
  return sections.some((s) => s.id === tab) ? (tab as Section) : 'library';
});
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="border-b border-border px-4 py-3">
      <h1 class="text-sm font-medium">Settings</h1>
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
          @click="router.replace({ query: { tab: section.id } })"
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
