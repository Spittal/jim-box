<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useLibraryStore } from '@/stores/library';

/**
 * Artwork of a collection track's file, loaded once it scrolls into view.
 * `thumb` asks for the small copy made for the track list.
 */
const props = defineProps<{ path: string | null; thumb?: boolean }>();

/** How long a list row must show a track before its thumbnail is fetched, so rows scrolled past fetch nothing. */
const SETTLE_MS = 120;

const library = useLibraryStore();
const el = ref<HTMLElement | null>(null);
const src = ref<string | null>(null);
const visible = ref(false);
let observer: IntersectionObserver | null = null;
let mounted = true;

async function load() {
  const path = props.path;
  src.value = null;
  if (!path || !visible.value) return;
  if (props.thumb && !library.hasCoverThumb(path)) {
    await new Promise((resolve) => setTimeout(resolve, SETTLE_MS));
    if (!mounted || props.path !== path) return;
  }
  const url = await (props.thumb ? library.coverThumb(path) : library.coverArt(path));
  if (props.path === path) src.value = url;
}

onMounted(() => {
  observer = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) {
      visible.value = true;
      observer?.disconnect();
      load();
    }
  }, { rootMargin: '200px' });
  if (el.value) observer.observe(el.value);
});
onBeforeUnmount(() => {
  mounted = false;
  observer?.disconnect();
});
watch(() => props.path, load);
</script>

<template>
  <div ref="el" class="shrink-0 overflow-hidden rounded bg-surface-2">
    <img v-if="src" :src="src" alt="" class="size-full object-cover" draggable="false" @error="src = null" />
  </div>
</template>
