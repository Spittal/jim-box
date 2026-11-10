<script setup lang="ts">
import { ref, watch } from 'vue';

const props = defineProps<{ src: string | null }>();

// Fall back to the placeholder if the image fails to load.
const failed = ref(false);
watch(() => props.src, () => (failed.value = false));
</script>

<template>
  <img
    v-if="src && !failed"
    :src="src"
    alt=""
    class="shrink-0 rounded bg-surface-2 object-cover"
    @error="failed = true"
  />
  <div v-else class="shrink-0 rounded bg-surface-2" />
</template>
