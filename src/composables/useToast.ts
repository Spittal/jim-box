import { ref } from 'vue';

/** How long a toast stays up. */
const TOAST_MS = 3500;

/** A word on what just happened, shown for a moment over the window's foot (see App.vue). */
export const toast = ref<string | null>(null);

let timer = 0;

/** Shows a toast in place of any that's up. */
export function showToast(text: string) {
  toast.value = text;
  clearTimeout(timer);
  timer = window.setTimeout(() => (toast.value = null), TOAST_MS);
}
