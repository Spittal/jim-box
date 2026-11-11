<script setup lang="ts">
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import SettingsSection from '@/components/SettingsSection.vue';
import SoulseekCredentialsForm from '@/components/SoulseekCredentialsForm.vue';
import { useSoulseekStore } from '@/stores/soulseek';
import { useSettingsStore } from '@/stores/settings';
import { useUiStore } from '@/stores/ui';
import { passwordStore } from '@/platform';

const soulseekStore = useSoulseekStore();
const { status } = storeToRefs(soulseekStore);
const { settings } = storeToRefs(useSettingsStore());
const ui = useUiStore();

/** Finding online was asked for with no account: say why this tab opened. */
const askToSignIn = computed(() => ui.soulseekPrompt && soulseekStore.needsCredentials);

const description = computed(() => {
  const s = status.value;
  switch (s.state) {
    case 'online':
      return `Connected as ${s.username}.`;
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
  return soulseekStore.reconnect().catch(() => {});
}
</script>

<template>
  <div class="flex flex-col gap-8">
    <p
      v-if="askToSignIn"
      role="status"
      class="max-w-xl rounded border border-accent bg-surface-1 px-3 py-2 text-xs text-text-primary"
    >
      Sign in to Soulseek to find tracks online. Tracks are fetched from the Soulseek network, so
      this needs an account: enter yours below, or pick a new username and password to create one.
    </p>

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
        :submit-label="soulseekStore.needsCredentials ? 'Sign in' : 'Save and reconnect'"
        :initial-username="settings?.soulseekUsername"
      />
    </SettingsSection>
  </div>
</template>
