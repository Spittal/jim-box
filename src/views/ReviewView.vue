<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import LibraryItemRow from '@/components/LibraryItemRow.vue';
import LibraryReview from '@/components/LibraryReview.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import { useLibraryStore } from '@/stores/library';
import { useSettingsStore } from '@/stores/settings';

/** Fixes proposed for collection tracks that were identified: review, then apply. */
const libraryStore = useLibraryStore();
const { items, progress, busy } = storeToRefs(libraryStore);
const { settings } = storeToRefs(useSettingsStore());
const musicDir = computed(() => settings.value?.musicDir ?? '');

const error = ref<string | null>(null);

const reviewItems = computed(() =>
  items.value.filter((i) => i.status === 'matched' || i.status === 'applying'),
);
/** Everything else: waiting, nothing to fix, no match, failed, done. */
const otherItems = computed(() =>
  items.value.filter((i) => i.status !== 'matched' && i.status !== 'applying'),
);
const settled = computed(() => otherItems.value.filter((i) => i.status !== 'pending').length);

const summary = computed(() => {
  const count = (status: string) => items.value.filter((i) => i.status === status).length;
  return [
    [reviewItems.value.length, 'to review'],
    [count('applied'), 'fixed'],
    [count('organized'), 'already fine'],
    [count('unmatched'), 'with no match'],
    [count('failed'), 'failed'],
  ]
    .filter(([n]) => n)
    .map(([n, label]) => `${n} ${label}`)
    .join(' · ');
});

const progressLabel = computed(() => {
  const { phase, done, total } = progress.value;
  const of = total ? ` ${done} of ${total}` : '…';
  switch (phase) {
    case 'scanning':
      return `Reading files${of}`;
    case 'matching':
      return `Identifying${of}`;
    case 'applying':
      return `Applying${of}`;
    case 'moving':
      return `Moving files${of}`;
    default:
      return '';
  }
});

async function run(action: () => Promise<unknown>) {
  error.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

const buttonClass =
  'rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3 disabled:opacity-50';
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex flex-col gap-2 border-b border-border px-4 py-3">
      <div class="flex items-center justify-between gap-4">
        <div class="min-w-0">
          <h1 class="text-sm font-medium">Review fixes</h1>
          <p class="truncate text-xs text-text-muted">
            {{ summary || 'Tags and file names proposed for tracks you identify in the collection.' }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <button v-if="busy" :class="buttonClass" @click="run(libraryStore.cancel)">Stop</button>
          <button
            v-else-if="settled"
            :class="buttonClass"
            title="Clear the results that need nothing more. Fixes still to review stay."
            @click="run(libraryStore.dismiss)"
          >
            Clear finished
          </button>
        </div>
      </div>
      <div v-if="busy" class="flex flex-col gap-1">
        <ProgressBar :percent="progress.total ? (progress.done / progress.total) * 100 : 0" />
        <span class="text-[11px] text-text-muted">{{ progressLabel }}</span>
      </div>
      <p v-if="error" class="error-box">{{ error }}</p>
    </header>

    <div v-if="!items.length" class="flex flex-col items-center gap-2 p-8 text-center text-sm text-text-muted">
      <p>Nothing to review.</p>
      <p class="max-w-md text-xs">
        Select tracks in the
        <router-link to="/collection" class="text-accent-hover hover:underline">collection</router-link>
        and choose Identify. Each is looked up on MusicBrainz, and the tags and file name it should
        have are proposed here. Applying them changes the files; the tracks keep their cues,
        grid and playlists in rekordbox.
      </p>
    </div>

    <template v-else>
      <LibraryReview
        v-if="reviewItems.length"
        :items="reviewItems"
        :music-dir="musicDir"
        :busy="busy"
        @error="error = $event"
      />
      <div
        v-if="otherItems.length"
        class="min-h-0 overflow-y-auto"
        :class="reviewItems.length ? 'max-h-[35%] shrink-0 border-t border-border' : 'flex-1'"
      >
        <h2 v-if="reviewItems.length" class="section-label px-4 pt-3 pb-1">Nothing to apply</h2>
        <ul>
          <LibraryItemRow
            v-for="item in otherItems"
            :key="item.id"
            :item="item"
            :music-dir="musicDir"
            :selectable="false"
            :selected="false"
          />
        </ul>
      </div>
    </template>
  </div>
</template>
