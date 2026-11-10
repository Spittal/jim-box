<script setup lang="ts">
defineProps<{ label: string; description?: string }>();
const model = defineModel<boolean>({ required: true });
</script>

<template>
  <label class="flex cursor-pointer items-start justify-between gap-4 rounded border border-border bg-surface-1 px-3 py-2">
    <span class="flex flex-col">
      <span class="text-sm">{{ label }}</span>
      <span v-if="description" class="text-xs text-text-muted">{{ description }}</span>
    </span>
    <input v-model="model" type="checkbox" class="mt-1 size-4 accent-accent" />
  </label>
</template>
