<script setup lang="ts">
defineProps<{ percent: number }>();
</script>

<template>
  <div class="h-1 overflow-hidden rounded-full bg-surface-3">
    <div class="h-full rounded-full bg-accent transition-[width] duration-500" :style="{ width: `${percent}%` }" />
  </div>
</template>
