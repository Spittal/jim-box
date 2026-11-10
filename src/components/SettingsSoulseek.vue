<script setup lang="ts">
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import SettingsSection from '@/components/SettingsSection.vue';
import SoulseekCredentialsForm from '@/components/SoulseekCredentialsForm.vue';
import { useDaemonStore } from '@/stores/daemon';
import { useSettingsStore } from '@/stores/settings';

const daemonStore = useDaemonStore();
const { status } = storeToRefs(daemonStore);
const { settings } = storeToRefs(useSettingsStore());
const error = ref<string | null>(null);

const description = computed(() => {
  const s = status.value;
  switch (s.state) {
    case 'online':
      return `Connected as ${s.username}.`;
    case 'starting':
      return 'Starting slskd…';
    case 'connecting':
      return s.detail ? `Connecting… (${s.detail})` : 'Connecting to the Soulseek network…';
    case 'error':
      return s.message;
    case 'needsCredentials':
      return 'No account set up.';
    default:
      return 'Not running.';
  }
});

async function reconnect() {
  error.value = null;
  try {
    await daemonStore.restart();
  } catch (e) {
    error.value = String(e);
  }
}
</script>

<template>
  <div class="flex flex-col gap-8">
    <SettingsSection title="Connection" :description="description" :error="error">
      <button
        class="self-start rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3"
        @click="reconnect"
      >
        Reconnect
      </button>
    </SettingsSection>

    <SettingsSection
      title="Account"
      description="Entering a new username and password creates a Soulseek account automatically. The password is kept in the macOS Keychain."
    >
      <SoulseekCredentialsForm
        submit-label="Save and reconnect"
        :initial-username="settings?.soulseekUsername"
      />
    </SettingsSection>
  </div>
</template>
