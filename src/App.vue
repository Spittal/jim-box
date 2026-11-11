<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { storeToRefs } from 'pinia';
import BottomBar from '@/components/BottomBar.vue';
import SearchPanel from '@/components/SearchPanel.vue';
import CollectionView from '@/views/CollectionView.vue';
import SettingsView from '@/views/SettingsView.vue';
import SetupView from '@/views/SetupView.vue';
import WelcomeView from '@/views/WelcomeView.vue';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import { useLibrariesStore } from '@/stores/libraries';
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
const librariesStore = useLibrariesStore();
const ui = useUiStore();
const { needsCredentials } = storeToRefs(daemonStore);
const { activeCount } = storeToRefs(jobsStore);

/** Whether the libraries have been asked for: until then there's nothing to show. */
const asked = ref(false);
/** A rekordbox library is open, at launch or once the welcome screen has opened one. */
const libraryOpen = computed(() => !!librariesStore.choices?.open);
const loaded = ref(false);

useEvents();

watch(libraryOpen, async (open) => {
  if (!open) return;
  // The library is read while the rest loads, so the list is there sooner.
  useCollectionStore().open();
  await Promise.all([settingsStore.load(), daemonStore.load(), jobsStore.load(), libraryStore.load()]);
  loaded.value = true;
});

onMounted(async () => {
  await librariesStore.load();
  asked.value = true;
});

// Dock badge: the number of tracks being fetched.
watch(
  activeCount,
  (count) => getCurrentWindow().setBadgeCount(count > 0 ? count : undefined).catch(() => {}),
  { immediate: true },
);

// Menu bar commands (see src-tauri/src/menu.rs).

async function focusSearchBar() {
  ui.settings = null;
  // The bar is hidden while settings are open.
  await nextTick();
  const input = document.querySelector<HTMLInputElement>('#search-bar');
  input?.focus();
  input?.select();
}

function runCommand(command: string) {
  if (!loaded.value || needsCredentials.value) return;
  if (command === 'settings') ui.toggleSettings();
  else if (command === 'consolidate') ui.showConsolidate();
  else if (command === 'find') void focusSearchBar();
  else if (command === 'find-online') {
    void focusSearchBar();
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
    <WelcomeView v-if="asked && !libraryOpen" />
    <template v-else-if="loaded && !needsCredentials">
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
