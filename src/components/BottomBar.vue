<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useBrowseStore } from '@/stores/browse';
import { useSoulseekStore } from '@/stores/soulseek';
import { useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { useLibrariesStore } from '@/stores/libraries';
import { usePlaylistStore } from '@/stores/playlist';
import { useSearchStore } from '@/stores/search';
import { useUiStore } from '@/stores/ui';
import { useUsbStore } from '@/stores/usb';

/**
 * The bar under the collection. Its box looks for tracks online, to add to
 * the collection and the playlist that's open. The rest says what is going
 * on in the background.
 */
const browse = useBrowseStore();
const { targetPlaylist } = storeToRefs(browse);
const searchStore = useSearchStore();
const { text: search } = storeToRefs(searchStore);
const playlistStore = usePlaylistStore();
const jobs = useJobsStore();
const library = useLibraryStore();
const ui = useUiStore();
const { status: connection } = storeToRefs(useSoulseekStore());
const usb = useUsbStore();
const libraries = useLibrariesStore();

/** The export to a stick under way, in a few words. */
const exportingUsb = computed(() => {
  if (!usb.exporting) return '';
  return usb.percent === null ? 'USB…' : `USB ${usb.percent}%`;
});

const input = ref<HTMLInputElement | null>(null);
const text = computed(() => search.value.trim());

function submit() {
  if (text.value) searchStore.run(text.value);
}

function escape() {
  if (searchStore.open) searchStore.close();
  else if (search.value) search.value = '';
  else input.value?.blur();
}

/** Where what's found goes: said in the field, as it's part of what the search is for. */
const destination = computed(() => targetPlaylist.value?.name ?? 'Collection');

// --- What's going on --------------------------------------------------------

/** The Downloads button: how many are on their way, and whether any stopped short. */
const downloads = computed(() => {
  const active = jobs.activeCount;
  const stuck = jobs.attentionCount;
  if (active) return { label: `Downloading ${active}`, dot: 'bg-accent animate-pulse', tone: 'text-text-primary', stuck };
  if (stuck) return { label: `Downloads · ${stuck} to check`, dot: 'bg-status-error', tone: 'text-status-error', stuck };
  return { label: 'Downloads', dot: '', tone: '', stuck };
});

const downloadsTitle = computed(() => {
  const parts = ['Downloads under way and finished'];
  if (downloads.value.stuck) parts.push(`${downloads.value.stuck} found no source or failed`);
  return parts.join('. ');
});

const identifying = computed(() => {
  const { phase, done, total } = library.progress;
  const of = total ? ` ${done} of ${total}` : '…';
  switch (phase) {
    case 'scanning':
      return `Reading files${of}`;
    case 'matching':
      return `Identifying${of}`;
    case 'applying':
      return `Applying fixes${of}`;
    case 'moving':
      return `Moving files${of}`;
    default:
      return '';
  }
});

/** Selects the tracks with a fix waiting, which shows the fixes beside the list. */
function reviewFixes() {
  browse.selectTracks(library.fixes.map((i) => i.trackId));
}

const soulseek = computed(() => {
  const s = connection.value;
  switch (s.state) {
    case 'online':
      return { dot: 'bg-status-ok', label: `Soulseek · ${s.username}` };
    case 'connecting':
      return { dot: 'bg-status-busy animate-pulse', label: 'Connecting to Soulseek…' };
    case 'error':
      return { dot: 'bg-status-error', label: 'Soulseek error' };
    case 'needsCredentials':
      return { dot: 'bg-status-idle', label: 'Sign in to Soulseek' };
    default:
      return { dot: 'bg-status-idle', label: 'Soulseek offline' };
  }
});

const chipClass =
  'shrink-0 rounded px-2 py-1 text-xs text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary';
</script>

<template>
  <footer class="flex shrink-0 items-center gap-2 border-t border-border bg-surface-1 px-3 py-2">
    <form class="flex min-w-0 flex-1 items-center gap-2" @submit.prevent="submit">
      <label
        class="relative flex min-w-0 flex-1 items-center overflow-hidden rounded border border-border bg-surface-0 focus-within:border-accent"
        :aria-busy="searchStore.loading"
      >
        <!-- Searching: a spinner where the text starts, and a line moving along the field's foot. -->
        <svg
          v-if="searchStore.loading"
          class="ml-2.5 size-3.5 shrink-0 animate-spin text-accent motion-reduce:animate-none"
          viewBox="0 0 16 16"
          fill="none"
          aria-hidden="true"
        >
          <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-opacity="0.25" stroke-width="2" />
          <path d="M14 8a6 6 0 0 0-6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
        </svg>
        <span v-if="searchStore.loading" class="absolute inset-x-0 bottom-0 h-0.5 overflow-hidden" aria-hidden="true">
          <span class="block h-full w-1/5 animate-row-busy bg-accent motion-reduce:w-full motion-reduce:animate-none motion-reduce:opacity-50" />
        </span>
        <input
          id="online-search"
          ref="input"
          v-model="search"
          type="search"
          spellcheck="false"
          placeholder="Find tracks online"
          class="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none placeholder:text-text-muted"
          :class="searchStore.loading ? 'pr-3 pl-2' : 'px-3'"
          @keydown.esc.prevent="escape"
        />
        <span
          class="mr-1.5 max-w-52 shrink-0 truncate rounded bg-surface-2 px-2 py-0.5 text-[11px] text-text-secondary"
          :title="targetPlaylist
            ? `What you get from the results goes into the collection and “${targetPlaylist.name}”. Open another playlist to add to it instead.`
            : 'What you get from the results goes into the collection. Open a playlist to add to it too.'"
        >
          Adding to {{ destination }}
        </span>
      </label>
      <button
        v-if="searchStore.loading"
        type="button"
        class="shrink-0 rounded bg-surface-2 px-3 py-1.5 text-xs font-medium text-text-primary transition-colors hover:bg-surface-3"
        :title="`Stop searching for “${searchStore.asked}” (Esc)`"
        @click="searchStore.cancel()"
      >
        Cancel
      </button>
      <button
        v-else
        type="submit"
        :disabled="!text"
        class="shrink-0 rounded bg-accent px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
        title="Look for this on Deezer and MusicBrainz, to add to your collection"
      >
        Find online
      </button>
    </form>

    <span v-if="playlistStore.importing" class="shrink-0 px-2 text-xs text-text-secondary">
      <template v-if="playlistStore.progress?.total">
        Matching tracks… {{ playlistStore.progress.done }} of {{ playlistStore.progress.total }}
      </template>
      <template v-else>Reading playlist…</template>
    </span>

    <span v-if="identifying" class="flex shrink-0 items-center gap-1 px-2 text-xs text-text-secondary">
      {{ identifying }}
      <button :class="chipClass" title="Stop. Files already changed stay changed." @click="library.cancel()">Stop</button>
    </span>
    <button
      v-else-if="library.fixes.length"
      :class="chipClass"
      title="Identifying found tags or file names to fix. Click to select those tracks and review the fixes."
      @click="reviewFixes"
    >
      {{ library.fixes.length }} {{ library.fixes.length === 1 ? 'fix' : 'fixes' }} to review
    </button>

    <button
      :class="[chipClass, downloads.tone, ui.downloads ? 'bg-surface-2' : '']"
      class="flex items-center gap-2"
      :title="downloadsTitle"
      @click="ui.downloads ? (ui.downloads = false) : ui.openDownloads()"
    >
      <span v-if="downloads.dot" class="size-2 shrink-0 rounded-full" :class="downloads.dot" />
      <span>{{ downloads.label }}</span>
      <span
        v-if="jobs.activeCount && downloads.stuck"
        class="rounded bg-status-error/15 px-1 font-mono text-[10px] leading-4 text-status-error"
        :title="`${downloads.stuck} found no source or failed`"
      >{{ downloads.stuck }}</span>
    </button>

    <button
      :class="chipClass"
      class="flex items-center gap-2"
      :title="connection.state === 'error' ? connection.message : 'Soulseek settings'"
      @click="ui.settings = 'soulseek'"
    >
      <span class="size-2 shrink-0 rounded-full" :class="soulseek.dot" />
      <span class="max-w-40 truncate">{{ soulseek.label }}</span>
    </button>
    <button
      :class="[chipClass, usb.exporting || libraries.stagedCount ? 'text-text-primary' : '']"
      class="flex items-center gap-1.5"
      :title="usb.exporting
        ? 'Writing to a USB stick. Click to see how it’s going.'
        : libraries.stagedCount
          ? `Changes waiting to be written to ${libraries.staged.map((s) => s.name).join(', ')}. Click to apply them.`
          : 'Export playlists to a USB stick, for CDJs and other players'"
      @click="ui.usb = !ui.usb"
    >
      <span v-if="libraries.stagedCount && !usb.exporting" class="size-2 shrink-0 rounded-full bg-accent" />
      {{ exportingUsb || (libraries.stagedCount ? `USB · ${libraries.stagedCount}` : 'USB') }}
    </button>
    <button :class="chipClass" title="Settings" @click="ui.toggleSettings()">Settings</button>
  </footer>
</template>
