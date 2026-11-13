<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import SettingsSection from '@/components/SettingsSection.vue';
import { useSettingsStore } from '@/stores/settings';
import { useSettingsSave } from '@/composables/useSettingsSave';
import { isMac } from '@/platform';
import type { CookieBrowser, YtDlpInfo } from '@/types/settings';

const { settings } = storeToRefs(useSettingsStore());
const { error, save } = useSettingsSave();

const tool = ref<YtDlpInfo | null>(null);
const toolError = ref<string | null>(null);

const allBrowsers: { id: CookieBrowser; label: string }[] = [
  { id: 'safari', label: 'Safari' },
  { id: 'chrome', label: 'Chrome' },
  { id: 'edge', label: 'Edge' },
  { id: 'firefox', label: 'Firefox' },
  { id: 'brave', label: 'Brave' },
  { id: 'vivaldi', label: 'Vivaldi' },
];
// Safari is only on macOS (see COOKIE_BROWSERS in sources/youtube.rs).
const browsers = allBrowsers.filter((b) => isMac || b.id !== 'safari');

onMounted(async () => {
  try {
    tool.value = await invoke<YtDlpInfo>('youtube_tool');
  } catch (e) {
    toolError.value = String(e);
  }
});

function setBrowser(value: string) {
  save({ youtubeCookiesBrowser: value ? (value as CookieBrowser) : null });
}
</script>

<template>
  <SettingsSection
    v-if="settings"
    title="YouTube Music"
    description="When Soulseek has no copy of a track, its download page offers to get it from YouTube Music instead. The audio is lossy AAC, saved as is, and a later search that finds a lossless copy replaces it."
    :error="error"
  >
    <div class="flex flex-col gap-1">
      <span class="text-xs text-text-secondary">yt-dlp</span>
      <p v-if="tool" class="font-mono text-xs text-text-muted select-text">{{ tool.version }} · {{ tool.path }}</p>
      <p v-else-if="toolError" class="error-box">{{ toolError }}</p>
      <p v-else class="text-xs text-text-muted">Looking for yt-dlp…</p>
      <p v-if="isMac" class="text-xs text-text-muted">Homebrew keeps it up to date with <span class="font-mono">brew upgrade yt-dlp</span>.</p>
      <p v-else class="text-xs text-text-muted">winget keeps it up to date with <span class="font-mono">winget upgrade yt-dlp</span>.</p>
    </div>

    <label class="flex flex-col gap-1">
      <span class="text-xs text-text-secondary">Sign in with cookies from</span>
      <select
        :value="settings.youtubeCookiesBrowser ?? ''"
        class="w-48 rounded border border-border bg-surface-1 px-2 py-1.5 text-xs outline-none focus:border-accent"
        @change="setBrowser(($event.target as HTMLSelectElement).value)"
      >
        <option value="">Don't sign in</option>
        <option v-for="b in browsers" :key="b.id" :value="b.id">{{ b.label }}</option>
      </select>
      <span class="text-xs text-text-muted">
        With YouTube Premium, pick the browser you're signed in to YouTube with to get 256 kbps instead of 128 kbps.
        <template v-if="isMac">
          Chrome, Edge, Brave and Vivaldi make macOS ask to allow access to their keychain item; choose Always Allow.
          Safari needs Full Disk Access for Jim Box in System Settings.
        </template>
        <template v-else>
          Firefox is the reliable choice on Windows: Chrome, Edge, Brave and Vivaldi lock their cookies to
          themselves, so yt-dlp often can't read them.
        </template>
      </span>
    </label>
  </SettingsSection>
</template>
