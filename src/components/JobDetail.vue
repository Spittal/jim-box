<script setup lang="ts">
import { computed, ref } from 'vue';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
import CoverArt from '@/components/CoverArt.vue';
import ProgressBar from '@/components/ProgressBar.vue';
import CandidateTable from '@/components/CandidateTable.vue';
import JobStatusBadge from '@/components/JobStatusBadge.vue';
import { isActive, useJobsStore } from '@/stores/jobs';
import { BUY_KIND_LABELS, useBuyLinks } from '@/composables/useBuyLinks';
import { basename, formatBytes, formatSpeed } from '@/composables/useFormat';
import { progressPercent } from '@/composables/useJobStatus';
import type { Job } from '@/types/job';

const props = defineProps<{ job: Job }>();
const emit = defineEmits<{ removed: [] }>();

const jobsStore = useJobsStore();
const actionError = ref<string | null>(null);

const meta = computed(() => props.job.metadata);
const active = computed(() => isActive(props.job));
const percent = computed(() => progressPercent(props.job));
const activeSource = computed(() =>
  props.job.activeCandidate === null ? null : props.job.candidates[props.job.activeCandidate],
);
// For tracks Soulseek hasn't found, including ones waiting to search again,
// with nothing in the library: a YouTube copy is lossy and shouldn't replace
// a file.
const canUseYoutube = computed(
  () => ['notFound', 'failed', 'waiting'].includes(props.job.status) && !props.job.replaces,
);
const youtubeHint = computed(
  () =>
    (props.job.status === 'waiting' ? 'Stop waiting for Soulseek and download' : 'Download') +
    ' it from YouTube Music as lossy AAC (256 kbps with Premium cookies, else 128 kbps). A later search that finds a lossless copy replaces it.',
);
const acceptedCount = computed(() => props.job.candidates.filter((c) => !c.rejected).length);
const rejectedCount = computed(() => props.job.candidates.length - acceptedCount.value);
// The album MusicBrainz tagged it from, once known: usually the studio album
// rather than a single.
const buyQuery = computed(() => ({
  title: meta.value?.title ?? props.job.request.title,
  artist: meta.value?.artist ?? props.job.request.artist,
  album: meta.value?.album ?? props.job.request.album,
  albumArtist: meta.value?.albumArtist ?? null,
  mbReleaseGroupId: meta.value?.mbReleaseGroupId ?? null,
}));
const { links, loading: linksLoading } = useBuyLinks(buyQuery);

const albumLine = computed(() => {
  const m = meta.value;
  if (!m) return props.job.request.album;
  const parts = [m.album];
  if (m.date) parts.push(m.date.slice(0, 4));
  if (m.trackNumber) parts.push(`Track ${m.trackNumber}${m.trackTotal ? ` of ${m.trackTotal}` : ''}`);
  return parts.join(' · ');
});

async function run(action: () => Promise<unknown>) {
  actionError.value = null;
  try {
    await action();
  } catch (e) {
    actionError.value = String(e);
  }
}

async function remove() {
  await run(() => jobsStore.remove(props.job.id));
  if (!actionError.value) emit('removed');
}
</script>

<template>
  <div class="flex flex-col gap-5 px-5 pb-5">
    <!-- Header -->
    <div class="flex gap-4">
      <CoverArt :src="job.request.coverXlUrl ?? job.request.coverUrl" class="size-20" />
      <div class="min-w-0 flex-1">
        <h2 class="truncate text-base font-semibold">{{ meta?.title ?? job.request.title }}</h2>
        <div class="truncate text-sm text-text-secondary">{{ meta?.artist ?? job.request.artist }}</div>
        <div class="truncate text-xs text-text-muted">{{ albumLine }}</div>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <JobStatusBadge :job="job" class="rounded border border-border px-2 py-0.5 text-xs text-text-secondary" />
          <button
            v-if="canUseYoutube"
            class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
            :title="youtubeHint"
            @click="run(() => jobsStore.downloadFromYoutube(job.id))"
          >
            Get from YouTube Music
          </button>
          <button
            v-if="active"
            class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
            @click="run(() => jobsStore.cancel(job.id))"
          >
            Cancel
          </button>
          <template v-else>
            <button
              v-if="job.status !== 'completed'"
              class="rounded bg-accent px-3 py-1 text-xs font-medium text-white transition-colors hover:bg-accent-hover"
              @click="run(() => jobsStore.retry(job.id))"
            >
              Search again
            </button>
            <button
              v-else
              class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
              :title="
                job.alreadyInLibrary
                  ? 'Download it anyway and replace the file in your library'
                  : 'Search again and replace this file, e.g. if it is the wrong song'
              "
              @click="run(() => jobsStore.redownload(job.id))"
            >
              {{ job.alreadyInLibrary ? 'Download anyway' : 'Download again' }}
            </button>
            <button
              v-if="job.outputPath"
              class="rounded bg-surface-2 px-3 py-1 text-xs transition-colors hover:bg-surface-3"
              @click="run(() => revealItemInDir(job.outputPath!))"
            >
              Show in Finder
            </button>
            <button
              class="rounded px-3 py-1 text-xs text-text-muted transition-colors hover:text-status-error"
              @click="remove"
            >
              Remove
            </button>
          </template>
        </div>
      </div>
    </div>

    <p v-if="actionError" class="error-box">{{ actionError }}</p>
    <p v-if="job.error" class="error-box">{{ job.error }}</p>

    <!-- Transfer -->
    <div v-if="job.status === 'downloading' && (activeSource || job.youtube)" class="flex flex-col gap-1.5">
      <ProgressBar :percent="percent ?? 0" />
      <div class="flex justify-between font-mono text-[11px] text-text-muted">
        <span v-if="activeSource" class="truncate">
          {{ basename(activeSource.filename) }} · {{ activeSource.username }}
        </span>
        <span v-else class="truncate">{{ job.youtube!.title }} · YouTube Music</span>
        <span v-if="job.progress && job.progress.bytesTransferred > 0" class="shrink-0 pl-2">
          {{ formatBytes(job.progress.bytesTransferred) }} / {{ formatBytes(job.progress.size) }}
          · {{ formatSpeed(job.progress.speed) }}
        </span>
      </div>
    </div>

    <div v-if="job.outputPath" class="flex flex-col gap-1">
      <h3 class="section-label">
        {{ job.alreadyInLibrary ? 'Already in your library' : 'Saved to' }}
      </h3>
      <p class="font-mono text-xs break-all text-text-muted select-text">{{ job.outputPath }}</p>
    </div>

    <!-- YouTube Music -->
    <div v-if="job.youtube" class="flex flex-col gap-1">
      <h3 class="section-label">YouTube Music</h3>
      <p class="text-xs text-text-muted">
        <button class="text-accent hover:text-accent-hover" @click="run(() => openUrl(job.youtube!.url))">
          {{ job.youtube.title }}</button
        ><template v-if="job.youtube.channel"> · {{ job.youtube.channel }}</template>
        <template v-if="job.youtube.bitRate"> · AAC {{ job.youtube.bitRate }} kbps</template>
      </p>
      <p v-if="job.status === 'completed' && job.youtube.bitRate && job.youtube.bitRate < 200" class="text-xs text-text-muted">
        YouTube gave the 128 kbps stream. With YouTube Premium, choose a browser to sign in with in Settings to get
        256 kbps.
      </p>
    </div>

    <!-- Searches -->
    <div v-if="job.searches.length && job.status !== 'completed'" class="flex flex-col gap-1">
      <h3 class="section-label">Searches</h3>
      <ul class="flex flex-col gap-0.5 font-mono text-[11px] text-text-muted">
        <li v-for="(search, i) in job.searches" :key="i" class="flex justify-between gap-3">
          <span class="truncate">“{{ search.query }}”</span>
          <span class="shrink-0">
            <template v-if="search.peers">
              {{ search.peers }} peers · {{ search.usable }} usable<template v-if="search.rejected">
                · {{ search.rejected }} other</template>
            </template>
            <template v-else>no peers answered</template>
          </span>
        </li>
      </ul>
      <p v-if="job.status === 'notFound' && job.searches.every((s) => !s.peers)" class="text-xs text-text-muted">
        Nobody answered, even after searching again for about half an hour. Soulseek was probably dropping
        searches. Try again later.
      </p>
      <p v-if="job.status === 'waiting'" class="text-xs text-text-muted">
        Nobody answered, which usually means Soulseek is dropping searches for a few minutes. This download will
        search again by itself.
      </p>
    </div>

    <!-- Metadata source -->
    <div v-if="meta" class="flex flex-col gap-1">
      <h3 class="section-label">Metadata</h3>
      <p class="text-xs text-text-muted">
        <template v-if="meta.mbRecordingId">
          Tagged from
          <button
            class="text-accent hover:text-accent-hover"
            @click="run(() => openUrl(`https://musicbrainz.org/recording/${meta!.mbRecordingId}`))"
          >
            MusicBrainz
          </button>
        </template>
        <template v-else>Not found on MusicBrainz, so it was tagged from Deezer's data.</template>
      </p>
    </div>

    <!-- Buy -->
    <div v-if="links.length || linksLoading" class="flex flex-col gap-1.5">
      <h3 class="section-label">
        {{ job.status === 'notFound' ? 'Buy a lossless copy' : 'Buy' }}
      </h3>
      <p v-if="linksLoading" class="text-xs text-text-muted">Looking for it in stores…</p>
      <div v-else class="flex flex-wrap gap-2">
        <button
          v-for="link in links"
          :key="link.store"
          class="rounded border border-border px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-border-active hover:text-text-primary"
          :title="link.url"
          @click="run(() => openUrl(link.url))"
        >
          {{ link.store }} <span class="text-text-muted">· {{ BUY_KIND_LABELS[link.kind] }}</span> ↗
        </button>
      </div>
    </div>

    <!-- Sources -->
    <div v-if="acceptedCount" class="flex flex-col gap-1.5">
      <h3 class="section-label">Soulseek sources ({{ acceptedCount }})</h3>
      <CandidateTable
        :candidates="job.candidates"
        :active-index="job.activeCandidate"
        :completed="job.status === 'completed'"
        :disabled="active"
        @download="(index) => run(() => jobsStore.downloadCandidate(job.id, index))"
      />
    </div>

    <details v-if="rejectedCount" class="flex flex-col gap-1.5" :open="!acceptedCount">
      <summary class="section-label cursor-pointer">Other files found ({{ rejectedCount }})</summary>
      <p class="mt-1.5 mb-1 text-xs text-text-muted">
        These lossless files have the right title but failed a check, so they're never downloaded automatically.
        Pick one if the check got it wrong.
      </p>
      <CandidateTable
        rejected
        :candidates="job.candidates"
        :active-index="job.activeCandidate"
        :completed="job.status === 'completed'"
        :disabled="active"
        @download="(index) => run(() => jobsStore.downloadCandidate(job.id, index))"
      />
    </details>
  </div>
</template>
