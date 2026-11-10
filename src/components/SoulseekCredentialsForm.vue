<script setup lang="ts">
import { ref } from 'vue';
import { useSettingsStore } from '@/stores/settings';

const props = defineProps<{ submitLabel: string; initialUsername?: string | null }>();

const settingsStore = useSettingsStore();
const username = ref(props.initialUsername ?? '');
const password = ref('');
const saving = ref(false);
const error = ref<string | null>(null);

async function submit() {
  saving.value = true;
  error.value = null;
  try {
    await settingsStore.setCredentials(username.value, password.value);
    password.value = '';
  } catch (e) {
    error.value = String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <form class="flex flex-col gap-3" @submit.prevent="submit">
    <label class="flex flex-col gap-1">
      <span class="text-xs text-text-secondary">Username</span>
      <input
        v-model="username"
        type="text"
        autocomplete="username"
        autocapitalize="off"
        spellcheck="false"
        required
        class="rounded border border-border bg-surface-0 px-3 py-1.5 text-sm outline-none focus:border-accent"
      />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-xs text-text-secondary">Password</span>
      <input
        v-model="password"
        type="password"
        autocomplete="current-password"
        required
        class="rounded border border-border bg-surface-0 px-3 py-1.5 text-sm outline-none focus:border-accent"
      />
    </label>
    <p v-if="error" class="error-box">{{ error }}</p>
    <button
      type="submit"
      :disabled="saving"
      class="mt-1 self-start rounded bg-accent px-4 py-1.5 text-sm font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
    >
      {{ saving ? 'Saving…' : props.submitLabel }}
    </button>
  </form>
</template>
