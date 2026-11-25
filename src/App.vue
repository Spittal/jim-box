<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { storeToRefs } from 'pinia';
import BottomBar from '@/components/BottomBar.vue';
import ConfirmDialog from '@/components/ConfirmDialog.vue';
import DownloadsModal from '@/components/DownloadsModal.vue';
import SearchPanel from '@/components/SearchPanel.vue';
import UsbExport from '@/components/UsbExport.vue';
import CollectionView from '@/views/CollectionView.vue';
import SettingsView from '@/views/SettingsView.vue';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useSoulseekStore } from '@/stores/soulseek';
import { useJobsStore } from '@/stores/jobs';
import { useLibrariesStore } from '@/stores/libraries';
import { useSettingsStore } from '@/stores/settings';
import { useLibraryStore } from '@/stores/library';
import { useSearchStore } from '@/stores/search';
import { useUiStore } from '@/stores/ui';
import { useEvents } from '@/composables/useEvents';
import { useFileDrop } from '@/composables/useFileDrop';
import { showToast, toast } from '@/composables/useToast';
import { commandKey, isMac } from '@/platform';

const soulseekStore = useSoulseekStore();
const jobsStore = useJobsStore();
const settingsStore = useSettingsStore();
const libraryStore = useLibraryStore();
const librariesStore = useLibrariesStore();
const ui = useUiStore();
const { needsCredentials } = storeToRefs(soulseekStore);
const { activeCount } = storeToRefs(jobsStore);

/** Whether the library has been asked after: until then there's nothing to show. */
const asked = ref(false);
/** The rekordbox library was opened at launch. */
const libraryOpen = computed(() => !!librariesStore.status?.open);
const loaded = ref(false);

useEvents();
/** Where files being dragged over the window would go. */
const dropInto = useFileDrop(() => loaded.value);

watch(libraryOpen, async (open) => {
  if (!open) return;
  // The library is read while the rest loads, so the list is there sooner.
  useCollectionStore().open();
  void librariesStore.refresh();
  void librariesStore.refreshPlugged();
  await Promise.all([settingsStore.load(), soulseekStore.load(), jobsStore.load(), libraryStore.load()]);
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

/** Puts the keyboard in a search field, out from under any dialog that would sit over it. */
async function focusField(id: string) {
  ui.settings = null;
  ui.usb = false;
  ui.downloads = false;
  await nextTick();
  const input = document.querySelector<HTMLInputElement>(`#${id}`);
  input?.focus();
  input?.select();
}

/** ⌘F: the list's search field, which shows itself. */
function findInList() {
  useBrowseStore().searchOpen = true;
  void focusField('library-search');
}

/** ⇧⌘F: the online search field, looking again for what's in it. */
function findOnline() {
  // With no Soulseek account, the sign-in comes first (as it does in the search itself).
  if (needsCredentials.value) return ui.promptSoulseek();
  void focusField('online-search');
  const search = useSearchStore();
  if (search.text.trim()) search.run(search.text);
}

/** Whether the keyboard is in something being typed, which has an undo of its own. */
function typing() {
  const at = document.activeElement;
  if (at instanceof HTMLInputElement) return !['checkbox', 'radio', 'range', 'button', 'color', 'file'].includes(at.type);
  return at instanceof HTMLTextAreaElement || (at instanceof HTMLElement && at.isContentEditable);
}

/** Undoes or redoes the last change to the library, and says what it was. */
async function undo(redo: boolean) {
  const what = await useCollectionStore().undo(redo, useBrowseStore().library);
  // A failure says so itself.
  if (what === undefined) return;
  showToast(
    what
      ? `${redo ? 'Redid' : 'Undid'} ${what}`
      : `Nothing to ${redo ? 'redo' : 'undo'}: only changes made since the app opened can be`,
  );
}

function runCommand(command: string) {
  if (!loaded.value) return;
  if (command === 'undo' || command === 'redo') {
    // The menu took the keys from the text field; this is what they'd have done there.
    if (typing()) document.execCommand(command);
    else void undo(command === 'redo');
  } else if (command === 'settings') ui.toggleSettings();
  else if (command === 'shortcuts') ui.toggleShortcuts();
  else if (command === 'consolidate') ui.showConsolidate();
  else if (command === 'find') findInList();
  else if (command === 'find-online') findOnline();
}

const unlistenMenu = listen<string>('menu', (e) => runCommand(e.payload));
onUnmounted(() => unlistenMenu.then((unlisten) => unlisten()));

// Windows has no menu bar to carry the shortcuts, so the window takes them.
// (On macOS the menu does, and handling them here too would run them twice.)
function onShortcut(event: KeyboardEvent) {
  if (!commandKey(event) || event.altKey) return;
  const key = event.key.toLowerCase();
  const command =
    key === '/'
      ? 'shortcuts'
      : key === 'f'
        ? event.shiftKey ? 'find-online' : 'find'
        : key === 'z'
          ? event.shiftKey ? 'redo' : 'undo'
          : key === 'y'
            ? 'redo'
            : null;
  if (!command) return;
  // A text field undoes its own typing.
  if ((command === 'undo' || command === 'redo') && typing()) return;
  // Also keeps the web view's own find bar from opening.
  event.preventDefault();
  runCommand(command);
}
if (!isMac) {
  onMounted(() => window.addEventListener('keydown', onShortcut));
  onUnmounted(() => window.removeEventListener('keydown', onShortcut));
}

/** `?` shows the shortcuts, as it does in many apps: no menu has it, so the window takes it everywhere. */
function onHelpKey(event: KeyboardEvent) {
  if (event.key !== '?' || event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
  const target = event.target as HTMLElement | null;
  if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
  event.preventDefault();
  runCommand('shortcuts');
}
onMounted(() => window.addEventListener('keydown', onHelpKey));
onUnmounted(() => window.removeEventListener('keydown', onHelpKey));
</script>

<template>
  <div class="flex h-full flex-col bg-surface-0">
    <!-- Titlebar drag region, under the traffic lights. Windows has a title bar of its own. -->
    <div v-if="isMac" data-tauri-drag-region class="h-8 shrink-0" />
    <!-- Most likely rekordbox isn't installed. There's nothing to manage without its library. -->
    <p v-if="asked && !libraryOpen" class="m-auto text-sm text-text-muted">
      Jim Box couldn’t open the rekordbox library on this computer.
    </p>
    <template v-else-if="loaded">
      <main class="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div class="min-h-0 flex-1">
          <CollectionView />
        </div>
        <SearchPanel />
        <SettingsView v-if="ui.settings" />
        <UsbExport v-if="ui.usb" />
        <DownloadsModal v-if="ui.downloads" />
      </main>
      <BottomBar />
    </template>
    <ConfirmDialog />
    <!-- A word on what just happened, gone in a moment. -->
    <Transition
      enter-active-class="transition duration-150"
      leave-active-class="transition duration-300"
      enter-from-class="translate-y-1 opacity-0"
      leave-to-class="opacity-0"
    >
      <div v-if="toast" class="pointer-events-none fixed inset-x-0 bottom-16 z-50 flex justify-center" role="status">
        <p class="rounded-full border border-border bg-surface-2 px-4 py-1.5 text-xs text-text-primary shadow-lg">
          {{ toast }}
        </p>
      </div>
    </Transition>
    <!-- Files dragged in from outside: says where a drop would put them. -->
    <div
      v-if="dropInto !== null"
      class="pointer-events-none fixed inset-0 z-50 flex items-end justify-center pb-16 outline outline-2 -outline-offset-2 outline-accent"
    >
      <p class="rounded-full bg-surface-2 px-4 py-1.5 text-xs text-text-primary shadow-lg">
        Drop to add to {{ dropInto ? `the collection and “${dropInto}”` : 'the collection' }}
      </p>
    </div>
  </div>
</template>
