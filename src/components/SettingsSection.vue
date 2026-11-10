<script setup lang="ts">
defineProps<{ title: string; description?: string; error?: string | null }>();
</script>

<template>
  <section class="flex max-w-xl flex-col gap-4">
    <div>
      <h2 class="text-sm font-medium">{{ title }}</h2>
      <p v-if="description" class="mt-1 text-xs text-text-muted">{{ description }}</p>
    </div>
    <slot />
    <p v-if="error" class="error-box">{{ error }}</p>
  </section>
</template>
