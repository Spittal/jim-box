<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import CollectionToolbar from '@/components/CollectionToolbar.vue';
import CollectionTrackDetail from '@/components/CollectionTrackDetail.vue';
import FetchPanel from '@/components/FetchPanel.vue';
import FixPanel from '@/components/FixPanel.vue';
import PlaylistTree from '@/components/PlaylistTree.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import TrackList from '@/components/TrackList.vue';
import TrackPlayer from '@/components/TrackPlayer.vue';
import { useBrowseStore } from '@/stores/browse';
import { useCollectionStore } from '@/stores/collection';
import { useLibraryStore } from '@/stores/library';
import { stepPane } from '@/composables/useDetailKeys';
import { useUiStore } from '@/stores/ui';
import { commandKey } from '@/platform';
import type { LibraryItem } from '@/types/library';

/** The one screen: rekordbox's playlists and tracks, with what's picked beside them. */
const store = useCollectionStore();
const { status, analysis, loading, error } = storeToRefs(store);
const browse = useBrowseStore();
const { selection, focused, detailTrack, notice, opened } = storeToRefs(browse);
const library = useLibraryStore();

onMounted(() => {
  if (!status.value && !loading.value) store.open();
  store.loadAnalysis();
});

/** The track in the player (`opened` is it as it was when opened) as the list now has it, when it's loaded: fresh after edits. */
const playerTrack = computed(() => (opened.value ? (browse.loadedTrack(opened.value.id) ?? opened.value) : null));

/** A track played to its end: the one under it in the list plays next. */
async function playNext() {
  const ended = opened.value;
  if (!ended) return;
  const next = await browse.trackAfter(ended.id).catch(() => null);
  // Unless another track was opened, or the player closed, meanwhile.
  if (next && opened.value === ended) opened.value = next;
}

/** What identifying found for the selected tracks, shown when no single track's details are. */
const identified = computed(() => {
  const items: LibraryItem[] = [];
  for (const id of selection.value) {
    const item = library.forTrack(id);
    if (item) items.push(item);
  }
  return items;
});

// --- The keyboard's pane --------------------------------------------------------

const ui = useUiStore();

/**
 * ⌘← and ⌘→ take the keyboard from pane to pane: playlists, tracks, sidebar.
 * And Tab stays in the pane it's in: the sidebar takes it round its entries,
 * and in the other two, which have nothing to type in, it does nothing rather
 * than wander off to a button somewhere else.
 */
function onKey(event: KeyboardEvent) {
  if (event.defaultPrevented || event.altKey || ui.settings) return;
  const target = event.target as HTMLElement | null;
  const pane = target?.closest('[data-pane]');
  const field = target?.closest<HTMLElement>('input, textarea, select, [contenteditable="true"]');
  if (event.key === 'Tab') {
    if (event.metaKey || event.ctrlKey || field || !(pane || target === document.body)) return;
  } else if (commandKey(event) && !event.shiftKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
    const left = event.key === 'ArrowLeft';
    if (field) {
      // A field elsewhere keeps the keys. One in a pane gives them up at the end of
      // its text they'd go to anyway, or with all of it selected, as it is when come to.
      if (!pane) return;
      if (field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) {
        const { selectionStart: start, selectionEnd: end, value } = field;
        const all = start === 0 && end === value.length;
        if (!all && (left ? end !== 0 : start !== value.length)) return;
      }
    }
    stepPane(target, left ? -1 : 1);
  } else return;
  event.preventDefault();
}
onMounted(() => window.addEventListener('keydown', onKey));
onUnmounted(() => window.removeEventListener('keydown', onKey));

const analysisPercent = computed(() =>
  analysis.value?.total ? (analysis.value.done / analysis.value.total) * 100 : 0,
);
const showFailures = ref(false);
/**
 * The banner is for batches: one track's analysis shows on its row. It goes
 * when the batch is done, unless there's something to read: tracks that
 * failed, or why it ended early.
 */
const showAnalysis = computed(() => {
  const batch = analysis.value;
  if (!batch) return false;
  if (batch.active) return batch.total > 1;
  return !!batch.stopped || (batch.total > 1 && batch.failed.length > 0);
});

const buttonClass =
  'rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <div class="flex h-full flex-col">
    <TrackPlayer :track="playerTrack" @close="opened = null" @ended="playNext" />
    <CollectionToolbar />

    <p v-if="error" class="error-box mx-4 mt-3">{{ error }}</p>
    <p v-else-if="notice" class="mx-4 mt-3 text-xs text-text-secondary">
      {{ notice }}
      <button class="ml-1 text-text-muted hover:text-text-primary" title="Dismiss" @click="notice = null">✕</button>
    </p>

    <div v-if="analysis && showAnalysis" class="mx-4 mt-3 flex flex-col gap-1.5 text-xs">
      <div class="flex items-center gap-3">
        <span class="shrink-0 text-text-secondary">
          <template v-if="analysis.active">Analysing {{ analysis.done }} of {{ analysis.total }}</template>
          <template v-else>Analysed {{ analysis.done - analysis.failed.length }} of {{ analysis.done }}</template>
        </span>
        <ProgressBar v-if="analysis.active" :percent="analysisPercent" class="flex-1" />
        <button
          v-if="analysis.failed.length"
          class="shrink-0 text-status-error hover:underline"
          @click="showFailures = !showFailures"
        >
          {{ analysis.failed.length }} failed
        </button>
        <button v-if="analysis.active" :class="buttonClass" class="shrink-0" @click="store.cancelAnalysis()">
          Stop
        </button>
        <button
          v-else
          class="text-text-muted hover:text-text-primary"
          title="Dismiss"
          @click="analysis = null"
        >✕</button>
      </div>
      <p v-if="analysis.stopped" class="text-status-busy">{{ analysis.stopped }}</p>
      <ul v-if="showFailures && analysis.failed.length" class="error-box max-h-32 overflow-y-auto">
        <li v-for="f in analysis.failed" :key="f.trackId">{{ f.title }}: {{ f.error }}</li>
      </ul>
    </div>

    <div v-if="status" class="flex min-h-0 flex-1">
      <PlaylistTree />
      <TrackList :playing-id="opened?.id ?? null" :playing-key="playerTrack?.key || null" @open="opened = $event" />
      <FetchPanel v-if="focused" />
      <CollectionTrackDetail v-else-if="detailTrack" :track="detailTrack" @close="browse.clearSelection()" />
      <FixPanel v-else-if="identified.length" :items="identified" @close="browse.clearSelection()" />
    </div>
  </div>
</template>
