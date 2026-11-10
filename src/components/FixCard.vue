<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useLibraryStore } from '@/stores/library';
import { useSettingsStore } from '@/stores/settings';
import type { LibraryItem } from '@/types/library';

/**
 * What identifying found for one track: the tags and file name it should
 * have, set against the ones it has, to apply or dismiss.
 */
const props = defineProps<{ item: LibraryItem }>();

const library = useLibraryStore();
const { settings } = storeToRefs(useSettingsStore());

const error = ref<string | null>(null);
const proposal = computed(() => props.item.proposal);

/** The tags that would change: [label, now, proposed]. */
const changes = computed(() => {
  const meta = proposal.value?.metadata;
  if (!meta) return [];
  const tags = props.item.tags;
  const text = (value: string | number | null | undefined) => (value === null || value === undefined ? '' : String(value));
  const rows: [string, string, string][] = [
    ['Title', text(tags.title), meta.title],
    ['Artist', text(tags.artist), meta.artist],
    ['Album', text(tags.album), meta.album],
    ['Album artist', text(tags.albumArtist), meta.albumArtist],
    ['Track', text(tags.trackNumber), text(meta.trackNumber)],
    ['Disc', text(tags.discNumber), text(meta.discNumber)],
    ['Date', text(tags.date), text(meta.date)],
  ];
  return rows.filter(([, now, proposed]) => proposed && now !== proposed);
});

/** Where the file would move, relative to its library folder; null when it stays. */
const newPath = computed(() => {
  const target = proposal.value?.targetPath;
  if (!target || target === props.item.path) return null;
  const roots = settings.value ? [settings.value.musicDir, ...settings.value.libraryDirs] : [];
  const root = roots.find((dir) => target.startsWith(dir) && '/\\'.includes(target[dir.length] ?? ''));
  return root ? target.slice(root.length + 1) : target;
});

async function run(action: () => Promise<unknown>) {
  error.value = null;
  try {
    await action();
  } catch (e) {
    error.value = String(e);
  }
}

const buttonClass = 'rounded px-2 py-0.5 text-[11px] transition-colors disabled:opacity-50';
</script>

<template>
  <div class="flex flex-col gap-1.5 rounded border border-border bg-surface-1 p-2 text-[11px]">
    <template v-if="item.status === 'matched' || item.status === 'applying'">
      <div class="flex items-center gap-1.5">
        <span class="section-label text-[10px]">Fix found</span>
        <span
          class="rounded px-1.5 font-mono text-[10px] leading-4"
          :class="proposal?.confidence === 'high' ? 'bg-accent/15 text-accent-hover' : 'bg-status-busy/15 text-status-busy'"
          :title="proposal?.confidence === 'high'
            ? 'A close match on MusicBrainz'
            : 'A likely match on MusicBrainz, but not a close one. Check it is the right track.'"
        >{{ proposal?.confidence === 'high' ? 'sure' : 'check' }}</span>
      </div>
      <p v-if="proposal?.album" class="text-text-muted">
        Matched with its album: {{ proposal.album.title }} · {{ proposal.album.matched }} of
        {{ proposal.album.tracks }} tracks
      </p>
      <dl class="flex flex-col gap-1">
        <div v-for="[label, now, proposed] in changes" :key="label">
          <dt class="text-text-muted">{{ label }}</dt>
          <dd class="break-words">
            <span v-if="now" class="text-text-muted line-through">{{ now }}</span>
            <span v-if="now" class="text-text-muted"> → </span>{{ proposed }}
          </dd>
        </div>
        <div v-if="newPath">
          <dt class="text-text-muted">Moves to</dt>
          <dd class="font-mono text-[10px] break-all text-text-secondary">{{ newPath }}</dd>
        </div>
      </dl>
      <p v-if="!changes.length && !newPath" class="text-text-muted">Only its MusicBrainz ids and artwork would change.</p>
      <div class="flex gap-1.5">
        <button
          :class="buttonClass"
          class="bg-accent font-medium text-white hover:bg-accent-hover"
          :disabled="library.busy || item.status === 'applying'"
          title="Write these tags to the file and move it. The track keeps its cues, grid and playlists. rekordbox must be closed."
          @click="run(() => library.apply([item.id]))"
        >
          {{ item.status === 'applying' ? 'Applying…' : 'Apply' }}
        </button>
        <button
          :class="buttonClass"
          class="bg-surface-2 hover:bg-surface-3"
          :disabled="item.status === 'applying'"
          title="Leave the file as it is"
          @click="run(() => library.dismiss([item.id]))"
        >
          Dismiss
        </button>
      </div>
    </template>
    <p v-else-if="item.status === 'pending'" class="text-text-muted">Identifying…</p>
    <p v-else-if="item.status === 'organized'" class="text-status-ok">Identified: its tags and file name are right.</p>
    <p v-else-if="item.status === 'applied'" class="text-status-ok">Tagged and filed.</p>
    <p v-else-if="item.status === 'unmatched'" class="text-text-muted">No confident match on MusicBrainz.</p>
    <p v-if="item.error" class="break-words text-status-error">{{ item.error }}</p>
    <p v-if="error" class="break-words text-status-error">{{ error }}</p>
  </div>
</template>
