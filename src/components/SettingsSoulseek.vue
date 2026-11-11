<script setup lang="ts">
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import SettingsSection from '@/components/SettingsSection.vue';
import SoulseekCredentialsForm from '@/components/SoulseekCredentialsForm.vue';
import { useDaemonStore } from '@/stores/daemon';
import { useSettingsStore } from '@/stores/settings';
import { passwordStore } from '@/platform';

const daemonStore = useDaemonStore();
const { status } = storeToRefs(daemonStore);
const { settings } = storeToRefs(useSettingsStore());

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

/** A failure becomes the connection's status, which the description shows. */
function reconnect() {
  return daemonStore.restart().catch(() => {});
}
</script>

<template>
  <div class="flex flex-col gap-8">
    <SettingsSection title="Connection" :description="description">
      <button
        class="self-start rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3"
        @click="reconnect"
      >
        Reconnect
      </button>
    </SettingsSection>

    <SettingsSection
      title="Account"
      :description="`Entering a new username and password creates a Soulseek account automatically. The password is kept in ${passwordStore}.`"
    >
      <SoulseekCredentialsForm
        submit-label="Save and reconnect"
        :initial-username="settings?.soulseekUsername"
      />
    </SettingsSection>
  </div>
</template>
