<script setup lang="ts">
import SoulseekCredentialsForm from '@/components/SoulseekCredentialsForm.vue';
import { passwordStore } from '@/platform';
</script>

<template>
  <div class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-8">
    <div class="w-full max-w-md">
      <h1 class="text-xl font-semibold">Welcome to jim-box</h1>
      <p class="mt-2 text-sm text-text-secondary">
        jim-box manages your rekordbox library, and finds lossless copies of tracks for it on the
        Soulseek network. Sign in with your Soulseek account, or pick a new username and password
        to create one.
      </p>
      <p class="mt-2 text-xs text-text-muted">
        Your password is stored in {{ passwordStore }}. Your music folder is shared back to the
        network by default, which you can change in Settings.
      </p>
      <div class="mt-6 rounded-lg border border-border bg-surface-1 p-5">
        <SoulseekCredentialsForm submit-label="Connect" />
      </div>
    </div>
  </div>
</template>
