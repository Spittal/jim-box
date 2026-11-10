<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { storeToRefs } from 'pinia';
import BottomBar from '@/components/BottomBar.vue';
import SearchPanel from '@/components/SearchPanel.vue';
import CollectionView from '@/views/CollectionView.vue';
import SettingsView from '@/views/SettingsView.vue';
import SetupView from '@/views/SetupView.vue';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import { useSettingsStore } from '@/stores/settings';
import { useLibraryStore } from '@/stores/library';
import { useSearchStore } from '@/stores/search';
import { useUiStore } from '@/stores/ui';
import { useEvents } from '@/composables/useEvents';
import { commandKey, isMac } from '@/platform';

const daemonStore = useDaemonStore();
const jobsStore = useJobsStore();
const settingsStore = useSettingsStore();
const libraryStore = useLibraryStore();
const ui = useUiStore();
const { needsCredentials } = storeToRefs(daemonStore);
const { activeCount } = storeToRefs(jobsStore);

const loaded = ref(false);

useEvents();

onMounted(async () => {
  // The library is read while the rest loads, so the list is there sooner.
  useCollectionStore().open();
  await Promise.all([settingsStore.load(), daemonStore.load(), jobsStore.load(), libraryStore.load()]);
  loaded.value = true;
});

// Dock badge: the number of tracks being fetched.
watch(
  activeCount,
  (count) => getCurrentWindow().setBadgeCount(count > 0 ? count : undefined).catch(() => {}),
  { immediate: true },
);

// Menu bar commands (see src-tauri/src/menu.rs).

function focusSearchBar() {
  ui.settings = null;
  const input = document.querySelector<HTMLInputElement>('#search-bar');
  input?.focus();
  input?.select();
}

function runCommand(command: string) {
  if (needsCredentials.value) return;
  if (command === 'settings') ui.toggleSettings();
  else if (command === 'find') focusSearchBar();
  else if (command === 'find-online') {
    focusSearchBar();
    useSearchStore().run(useBrowseStore().search);
  }
}

const unlistenMenu = listen<string>('menu', (e) => runCommand(e.payload));
onUnmounted(() => unlistenMenu.then((unlisten) => unlisten()));

// Windows has no menu bar to carry the shortcuts, so the window takes them.
// (On macOS the menu does, and handling them here too would run them twice.)
function onShortcut(event: KeyboardEvent) {
  if (!commandKey(event) || event.altKey) return;
  const key = event.key.toLowerCase();
  const command = key === ',' ? 'settings' : key === 'f' ? (event.shiftKey ? 'find-online' : 'find') : null;
  if (!command) return;
  // Also keeps the web view's own find bar from opening.
  event.preventDefault();
  runCommand(command);
}
if (!isMac) {
  onMounted(() => window.addEventListener('keydown', onShortcut));
  onUnmounted(() => window.removeEventListener('keydown', onShortcut));
}
</script>

<template>
  <div class="flex h-full flex-col bg-surface-0">
    <!-- Titlebar drag region, under the traffic lights. Windows has a title bar of its own. -->
    <div v-if="isMac" data-tauri-drag-region class="h-8 shrink-0" />
    <template v-if="loaded && !needsCredentials">
      <main class="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div class="min-h-0 flex-1">
          <CollectionView />
        </div>
        <SearchPanel />
        <SettingsView v-if="ui.settings" />
      </main>
      <BottomBar />
    </template>
    <SetupView v-else-if="loaded" />
  </div>
</template>
