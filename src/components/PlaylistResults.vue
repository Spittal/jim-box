<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import CoverArt from '@/components/CoverArt.vue';
import PlaylistEntryRow from '@/components/PlaylistEntryRow.vue';
import { useJobsStore } from '@/stores/jobs';
import { usePlaylistStore } from '@/stores/playlist';
import { formatAgo } from '@/composables/useFormat';
import type { PlaylistEntry, PlaylistSync } from '@/types/playlist';

defineProps<{ activeJobId: string | null; activeEntry: number | null }>();
const emit = defineEmits<{ open: [index: number] }>();

const jobsStore = useJobsStore();
const playlistStore = usePlaylistStore();
const { playlist } = storeToRefs(playlistStore);

const starting = ref(false);
const error = ref<string | null>(null);

const jobOf = (e: PlaylistEntry) => (e.request ? jobsStore.jobFor(e.request) : undefined);
const hasJob = (e: PlaylistEntry) => !!jobOf(e);

// Confident matches without a download yet are selected by default;
// `toggled` records the user's deviations from that.
const toggled = ref(new Set<number>());
function isSelected(e: PlaylistEntry, index: number): boolean {
  const byDefault = e.confidence === 'high' && !hasJob(e);
  return byDefault !== toggled.value.has(index);
}
function toggle(index: number) {
  const next = new Set(toggled.value);
  if (!next.delete(index)) next.add(index);
  toggled.value = next;
}

const entries = computed(() => playlist.value?.entries ?? []);
const matched = computed(() => entries.value.filter((e) => e.request).length);
const selected = computed(() =>
  entries.value.filter((e, i) => e.request && !hasJob(e) && isSelected(e, i)),
);

// Whether downloads from here join a rekordbox playlist of the same name.
const toRekordbox = ref(true);
const syncing = ref(false);
const synced = ref<PlaylistSync | null>(null);

/** Puts the tracks already in the collection into the rekordbox playlist, in order. */
async function syncNow() {
  if (!playlist.value) return;
  syncing.value = true;
  error.value = null;
  synced.value = null;
  try {
    synced.value = await playlistStore.syncToRekordbox(playlist.value.id);
  } catch (e) {
    error.value = String(e);
  } finally {
    syncing.value = false;
  }
}

async function downloadSelected() {
  starting.value = true;
  error.value = null;
  try {
    const into = toRekordbox.value ? playlist.value?.name : undefined;
    // Jobs queue in the backend (a few run at a time), so starting them all is fine.
    for (const entry of selected.value) {
      await jobsStore.start(entry.request!, undefined, into);
    }
    toggled.value = new Set();
  } catch (e) {
    error.value = String(e);
  } finally {
    starting.value = false;
  }
}
</script>

<template>
  <div v-if="playlist" class="flex min-h-0 flex-1 flex-col">
    <div class="flex items-center gap-3 border-b border-border px-4 py-3">
      <CoverArt :src="playlist.coverUrl" class="size-12" />
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm font-medium">{{ playlist.name }}</div>
        <div class="text-xs text-text-muted">
          {{ entries.length }} tracks · {{ matched }} matched · loaded {{ formatAgo(playlist.loadedAt) }}
          <template v-if="playlist.truncated">
            · Spotify only shares the first 100 tracks of a playlist without signing in
          </template>
        </div>
      </div>
      <button
        class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
        title="Read the playlist from Spotify again to pick up added tracks. Existing matches are kept."
        @click="playlistStore.refresh()"
      >
        Refresh
      </button>
      <button
        class="shrink-0 rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50"
        :disabled="syncing"
        title="Make a rekordbox playlist with this name and put in it the tracks you already have, in this order. Tracks already in it stay where they are. rekordbox must be closed."
        @click="syncNow"
      >
        {{ syncing ? 'Copying…' : 'Copy to rekordbox' }}
      </button>
    </div>
    <p v-if="synced" class="border-b border-border px-4 py-2 text-xs text-text-secondary">
      “{{ synced.name }}” in rekordbox has the {{ synced.present }}
      {{ synced.present === 1 ? 'track' : 'tracks' }} you have.
      <template v-if="synced.missing">{{ synced.missing }} aren't in your collection yet.</template>
    </p>

    <ul class="min-h-0 flex-1 overflow-y-auto">
      <PlaylistEntryRow
        v-for="(entry, index) in entries"
        :key="index"
        :entry="entry"
        :selected="isSelected(entry, index)"
        :active="activeEntry === index || (!!activeJobId && jobOf(entry)?.id === activeJobId)"
        @toggle="toggle(index)"
        @open="emit('open', index)"
      />
    </ul>

    <footer class="flex items-center justify-between gap-3 border-t border-border px-4 py-2">
      <span class="min-w-0 flex-1 truncate text-xs text-status-error" :title="error ?? ''">{{ error }}</span>
      <label
        class="flex shrink-0 items-center gap-1.5 text-xs text-text-secondary"
        title="Each track joins a rekordbox playlist with this playlist's name as it finishes downloading"
      >
        <input v-model="toRekordbox" type="checkbox" class="size-3.5 accent-accent" />
        Add to a rekordbox playlist
      </label>
      <button
        :disabled="starting || !selected.length"
        class="rounded bg-accent px-4 py-1.5 text-xs font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
        @click="downloadSelected"
      >
        {{ starting ? 'Queuing…' : `Get lossless for ${selected.length} ${selected.length === 1 ? 'track' : 'tracks'}` }}
      </button>
    </footer>
  </div>
</template>
