<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { storeToRefs } from 'pinia';
import DaemonStatusBar from '@/components/DaemonStatusBar.vue';
import SetupView from '@/views/SetupView.vue';
import { useDaemonStore } from '@/stores/daemon';
import { useJobsStore } from '@/stores/jobs';
import { useSettingsStore } from '@/stores/settings';
import { useLibraryStore } from '@/stores/library';
import { useEvents } from '@/composables/useEvents';

const daemonStore = useDaemonStore();
const jobsStore = useJobsStore();
const settingsStore = useSettingsStore();
const libraryStore = useLibraryStore();
const { needsCredentials } = storeToRefs(daemonStore);
const { activeCount } = storeToRefs(jobsStore);
const { toReview } = storeToRefs(libraryStore);

const loaded = ref(false);

useEvents();

onMounted(async () => {
  await Promise.all([settingsStore.load(), daemonStore.load(), jobsStore.load(), libraryStore.load()]);
  loaded.value = true;
});

// Dock badge: the number of downloads in progress.
watch(
  activeCount,
  (count) => getCurrentWindow().setBadgeCount(count > 0 ? count : undefined).catch(() => {}),
  { immediate: true },
);

// Menu bar commands (see src-tauri/src/menu.rs).
const router = useRouter();
const route = useRoute();

async function focusSearchField() {
  // The current view's search field if it has one (Search, Collection), else Search's.
  let input = document.querySelector<HTMLInputElement>('main input[type="search"]');
  if (!input) {
    await router.push('/search');
    await nextTick();
    input = document.querySelector<HTMLInputElement>('main input[type="search"]');
  }
  input?.focus();
  input?.select();
}

const menuRoutes: Record<string, string> = {
  settings: '/settings',
  'go-search': '/search',
  'go-library': '/collection',
  'go-downloads': '/downloads',
};

const unlistenMenu = listen<string>('menu', (e) => {
  if (needsCredentials.value) return;
  if (e.payload === 'find') focusSearchField();
  else if (menuRoutes[e.payload] && !route.path.startsWith(menuRoutes[e.payload])) {
    router.push(menuRoutes[e.payload]);
  }
});
onUnmounted(() => unlistenMenu.then((unlisten) => unlisten()));

// Sidebar rows, styled like a macOS source list over the translucent sidebar.
const navClass =
  'mx-2 flex items-center justify-between rounded-md px-2.5 py-1 text-sm text-text-secondary hover:bg-white/5 hover:text-text-primary';
const navActiveClass = 'bg-white/10 text-text-primary';
</script>

<template>
  <div class="flex h-full">
    <template v-if="loaded && !needsCredentials">
      <!-- Translucent sidebar: the window's vibrancy shows through. -->
      <aside class="flex w-52 shrink-0 flex-col border-r border-black/40">
        <!-- Titlebar drag region, under the traffic lights -->
        <div data-tauri-drag-region class="h-8 shrink-0" />
        <div data-tauri-drag-region class="px-4 pt-1 pb-2">
          <span class="pointer-events-none font-mono text-xs font-medium tracking-wide text-text-secondary uppercase">
            jim-box
          </span>
        </div>
        <nav class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto py-1">
          <router-link to="/search" :class="navClass" :active-class="navActiveClass">
            Search
          </router-link>
          <router-link to="/collection" :class="navClass" :active-class="navActiveClass">
            Collection
          </router-link>
          <router-link to="/review" :class="navClass" :active-class="navActiveClass">
            Review fixes
            <span
              v-if="toReview"
              class="rounded-full bg-white/10 px-1.5 font-mono text-[11px] leading-5 text-text-secondary"
            >{{ toReview }}</span>
          </router-link>
          <router-link to="/downloads" :class="navClass" :active-class="navActiveClass">
            Downloads
            <span
              v-if="activeCount"
              class="rounded-full bg-accent px-1.5 font-mono text-[11px] leading-5 text-white"
            >{{ activeCount }}</span>
          </router-link>
        </nav>

        <div class="shrink-0 border-t border-white/5 py-1">
          <DaemonStatusBar />
          <router-link
            to="/settings"
            class="mx-2 flex items-center gap-2 rounded-md px-2.5 py-1.5 text-xs text-text-secondary hover:bg-white/5 hover:text-text-primary"
            :active-class="navActiveClass"
          >
            Settings
          </router-link>
        </div>
      </aside>

      <div class="flex min-w-0 flex-1 flex-col bg-surface-0">
        <!-- Titlebar drag region over the content -->
        <div data-tauri-drag-region class="h-8 shrink-0" />
        <main class="min-h-0 min-w-0 flex-1 overflow-hidden">
          <router-view />
        </main>
      </div>
    </template>

    <div v-else class="flex min-w-0 flex-1 flex-col bg-surface-0">
      <div data-tauri-drag-region class="h-8 shrink-0" />
      <SetupView v-if="loaded" />
    </div>
  </div>
</template>
