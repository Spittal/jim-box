<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useLibraryStore } from '@/stores/library';

/** Artwork of a collection track's file, loaded once it scrolls into view. */
const props = defineProps<{ path: string | null }>();

const library = useLibraryStore();
const el = ref<HTMLElement | null>(null);
const src = ref<string | null>(null);
const visible = ref(false);
let observer: IntersectionObserver | null = null;

async function load() {
  const path = props.path;
  src.value = null;
  if (!path || !visible.value) return;
  const url = await library.coverArt(path);
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
onBeforeUnmount(() => observer?.disconnect());
watch(() => props.path, load);
</script>

<template>
  <div ref="el" class="shrink-0 overflow-hidden rounded bg-surface-2">
    <img v-if="src" :src="src" alt="" class="size-full object-cover" @error="src = null" />
  </div>
</template>
