<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useBrowseStore } from '@/stores/browse';
import { useSoulseekStore } from '@/stores/soulseek';
import { useJobsStore } from '@/stores/jobs';
import { useLibraryStore } from '@/stores/library';
import { usePlaylistStore } from '@/stores/playlist';
import { useSearchStore } from '@/stores/search';
import { useUiStore } from '@/stores/ui';

/**
 * The bar under the collection. Its box filters the list as you type. On
 * Return it looks for the text online instead, and the list is whole again.
 * The rest says what is going on in the background.
 */
const browse = useBrowseStore();
const { search, selectedNode, targetPlaylist, pendingOnly } = storeToRefs(browse);
const searchStore = useSearchStore();
const playlistStore = usePlaylistStore();
const jobs = useJobsStore();
const library = useLibraryStore();
const ui = useUiStore();
const { status: connection } = storeToRefs(useSoulseekStore());

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

const placeholder = computed(
  () =>
    `Filter ${selectedNode.value?.name ?? (browse.lists.length > 1 ? 'these playlists' : 'your collection')}, or press Return to find tracks online`,
);

// --- What's going on --------------------------------------------------------

const fetching = computed(() => {
  const active = jobs.activeCount;
  const stopped = jobs.pending.length - active;
  const parts = [];
  if (active) parts.push(`${active} fetching`);
  if (stopped) parts.push(`${stopped} not found`);
  return parts.join(' · ');
});

function togglePending() {
  if (pendingOnly.value) pendingOnly.value = false;
  else {
    browse.select(null);
    pendingOnly.value = true;
  }
}

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
      <input
        id="search-bar"
        ref="input"
        v-model="search"
        type="search"
        spellcheck="false"
        :placeholder="placeholder"
        class="min-w-0 flex-1 rounded border border-border bg-surface-0 px-3 py-1.5 text-sm outline-none placeholder:text-text-muted focus:border-accent"
        @keydown.esc.prevent="escape"
      />
      <button
        type="submit"
        :disabled="!text || searchStore.loading"
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
    <span
      v-else
      class="max-w-52 shrink-0 truncate px-2 text-xs text-text-muted"
      title="Where a track you fetch from the results goes. Open a playlist to fetch into it."
    >
      Fetches go to {{ targetPlaylist?.name ?? 'the collection' }}
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

    <template v-if="fetching">
      <button
        :class="[chipClass, pendingOnly ? 'bg-surface-3 text-text-primary' : '']"
        title="Tracks on their way into the collection. Click to show only these."
        @click="togglePending"
      >
        {{ fetching }}
      </button>
      <button
        v-if="pendingOnly && jobs.notFoundCount"
        :class="chipClass"
        title="Search Soulseek again for every track that found no source"
        @click="jobs.retryNotFound()"
      >
        Search again for {{ jobs.notFoundCount }}
      </button>
    </template>

    <button
      :class="chipClass"
      class="flex items-center gap-2"
      :title="connection.state === 'error' ? connection.message : 'Soulseek settings'"
      @click="ui.settings = 'soulseek'"
    >
      <span class="size-2 shrink-0 rounded-full" :class="soulseek.dot" />
      <span class="max-w-40 truncate">{{ soulseek.label }}</span>
    </button>
    <button :class="chipClass" title="Settings" @click="ui.toggleSettings()">Settings</button>
  </footer>
</template>
