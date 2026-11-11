<script setup lang="ts">
import LibraryChooser from '@/components/LibraryChooser.vue';
</script>

<template>
  <div class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-8">
    <div class="w-full max-w-lg">
      <h1 class="text-xl font-semibold">Welcome to jim-box</h1>
      <p class="mt-2 text-sm text-text-secondary">
        Point jim-box at the rekordbox library you want to manage.
      </p>
      <p class="mt-2 text-xs text-text-muted">
        A library is a rekordbox <code class="font-mono">master.db</code>: the one on this computer,
        one on a drive, or any other. Each keeps its own music folders, downloads and settings
        here, and you can switch between them in Settings.
      </p>
      <div class="mt-6 rounded-lg border border-border bg-surface-1 p-5">
        <LibraryChooser />
      </div>
    </div>
  </div>
</template>
