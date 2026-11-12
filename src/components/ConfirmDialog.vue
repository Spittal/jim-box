<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { asking } from '@/composables/useConfirm';

/** The question `confirm()` is asking, in a dialog over everything else. */
const ticked = ref(false);
const confirmButton = ref<HTMLButtonElement | null>(null);

watch(asking, async (question) => {
  if (!question) return;
  ticked.value = question.option?.checked ?? false;
  await nextTick();
  // Return goes ahead, as it does in the system's own dialogs.
  confirmButton.value?.focus();
});

// Keys are the dialog's alone while it's up: Escape says no, and the list and
// the dialogs under it hear nothing.
function onKey(event: KeyboardEvent) {
  if (!asking.value) return;
  event.stopPropagation();
  if (event.key !== 'Escape') return;
  event.preventDefault();
  asking.value.answer(null);
}
onMounted(() => window.addEventListener('keydown', onKey, true));
onUnmounted(() => window.removeEventListener('keydown', onKey, true));
</script>

<template>
  <Teleport to="body">
    <div
      v-if="asking"
      class="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-6"
      @click.self="asking.answer(null)"
    >
      <div
        role="alertdialog"
        aria-modal="true"
        :aria-label="asking.title"
        class="flex w-full max-w-sm flex-col gap-3 rounded-lg border border-border bg-surface-1 p-5 shadow-xl"
      >
        <h2 class="text-sm font-medium break-words">{{ asking.title }}</h2>
        <p v-if="asking.message" class="text-xs leading-relaxed whitespace-pre-line text-text-secondary">
          {{ asking.message }}
        </p>
        <label v-if="asking.option" class="flex cursor-pointer items-center gap-2 text-xs">
          <input v-model="ticked" type="checkbox" class="size-3.5 accent-accent" />
          {{ asking.option.label }}
        </label>
        <div class="mt-1 flex justify-end gap-2">
          <button
            class="rounded bg-surface-2 px-3 py-1.5 text-xs transition-colors hover:bg-surface-3"
            @click="asking.answer(null)"
          >
            Cancel
          </button>
          <button
            ref="confirmButton"
            class="rounded px-3 py-1.5 text-xs font-medium text-white outline-offset-2 transition-colors focus-visible:outline focus-visible:outline-2"
            :class="asking.danger
              ? 'bg-status-error outline-status-error hover:bg-status-error/80'
              : 'bg-accent outline-accent hover:bg-accent-hover'"
            @click="asking.answer({ option: ticked })"
          >
            {{ asking.confirm }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
