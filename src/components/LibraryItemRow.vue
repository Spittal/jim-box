<script setup lang="ts">
import { computed, ref } from 'vue';
import { useJobsStore } from '@/stores/jobs';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import type { LibraryItem } from '@/types/library';

const props = defineProps<{
  item: LibraryItem;
  musicDir: string;
  selectable: boolean;
  selected: boolean;
}>();
const emit = defineEmits<{ toggle: [] }>();

const jobsStore = useJobsStore();
const startError = ref<string | null>(null);

const proposal = computed(() => props.item.proposal);
const meta = computed(() => proposal.value?.metadata);

const quality = computed(() => {
  const { format, bitDepth, sampleRate, bitrate, lossless } = props.item;
  if (lossless && bitDepth && sampleRate) return `${format} ${bitDepth}/${+(sampleRate / 1000).toFixed(1)}`;
  if (!lossless && bitrate) return `${format} ${bitrate}k`;
  return format;
});

const currentLabel = computed(() => {
  const { title, artist } = props.item.tags;
  return title ? [artist, title].filter(Boolean).join(' – ') : 'No tags';
});

/** Target path relative to the music folder, if the file will move. */
const newPath = computed(() => {
  const target = proposal.value?.targetPath;
  if (!target || target === props.item.path) return null;
  return target.startsWith(props.musicDir) ? target.slice(props.musicDir.length + 1) : target;
});

const upgradeJob = computed(() => {
  const request = proposal.value?.request;
  return request ? jobsStore.jobFor(request) : undefined;
});

async function getLossless() {
  const request = proposal.value?.request;
  if (!request) return;
  startError.value = null;
  try {
    await jobsStore.start(request, props.item.path);
  } catch (e) {
    startError.value = String(e);
  }
}
</script>

<template>
  <li class="flex items-start gap-3 border-b border-border px-4 py-2.5">
    <input
      v-if="selectable"
      type="checkbox"
      class="mt-1 size-3.5 shrink-0 accent-accent"
      :checked="selected"
      :disabled="item.status !== 'matched'"
      @change="emit('toggle')"
    />

    <div class="grid min-w-0 flex-1 grid-cols-2 gap-4">
      <!-- Current -->
      <div class="min-w-0">
        <div class="truncate text-xs text-text-secondary" :title="item.relativePath">{{ item.relativePath }}</div>
        <div class="truncate text-xs text-text-muted">{{ currentLabel }}</div>
        <div class="mt-0.5 flex gap-1.5">
          <span
            class="rounded px-1.5 font-mono text-[10px] leading-4"
            :class="item.lossless ? 'bg-status-ok/15 text-status-ok' : 'bg-surface-2 text-text-muted'"
          >{{ quality }}</span>
        </div>
      </div>

      <!-- Proposed -->
      <div class="min-w-0">
        <template v-if="meta">
          <div class="flex items-center gap-1.5">
            <span class="truncate text-sm">{{ meta.title }}</span>
            <span
              v-if="item.status === 'matched'"
              class="shrink-0 rounded px-1.5 font-mono text-[10px] leading-4"
              :class="proposal?.confidence === 'high' ? 'bg-accent/15 text-accent-hover' : 'bg-status-busy/15 text-status-busy'"
            >{{ proposal?.confidence === 'high' ? 'sure' : 'check' }}</span>
          </div>
          <div class="truncate text-xs text-text-muted">
            {{ meta.artist }} · {{ meta.album }}{{ meta.date ? ` (${meta.date.slice(0, 4)})` : '' }}
          </div>
          <div v-if="newPath" class="truncate font-mono text-[11px] text-text-muted" :title="newPath">
            → {{ newPath }}
          </div>
        </template>
        <div v-else-if="item.status === 'organized'" class="text-xs text-status-ok">Nothing to fix</div>
        <div v-else-if="item.status === 'unmatched'" class="text-xs text-text-muted">No confident match found</div>
        <div v-else-if="item.status === 'pending'" class="text-xs text-text-muted">Waiting…</div>

        <div v-if="item.status === 'applying'" class="text-xs text-status-busy">Applying…</div>
        <div v-if="item.status === 'applied'" class="text-xs text-status-ok">Tagged and filed</div>
        <div v-if="item.error" class="text-xs break-words text-status-error">{{ item.error }}</div>

        <!-- Lossy upgrade -->
        <div v-if="!item.lossless && proposal?.request" class="mt-1">
          <router-link
            v-if="upgradeJob"
            :to="`/downloads/${upgradeJob.id}`"
            class="inline-flex text-[11px] text-text-secondary hover:text-text-primary"
          >
            <JobStatusBadge :job="upgradeJob" />
          </router-link>
          <button
            v-else
            class="rounded bg-surface-2 px-2 py-0.5 text-[11px] transition-colors hover:bg-surface-3"
            title="Download a lossless copy; this file goes to the Trash once it succeeds"
            @click="getLossless"
          >
            Get lossless
          </button>
          <span v-if="startError" class="ml-2 text-[11px] text-status-error">{{ startError }}</span>
        </div>
      </div>
    </div>
  </li>
</template>
