import { shallowRef } from 'vue';

/** Something to ask before going ahead, in a dialog over the window (see ConfirmDialog.vue). */
export interface Confirmation {
  title: string;
  /** What going ahead does, where the title doesn't say it all. */
  message?: string;
  /** The label of the button that goes ahead. */
  confirm: string;
  /** Going ahead removes something. */
  danger?: boolean;
  /** A choice about how to go ahead, as a tick box. */
  option?: { label: string; checked: boolean };
}

/** The answer when it's yes: whether the option was ticked. */
export interface Confirmed {
  option: boolean;
}

/** The question being asked, and where its answer goes. */
export const asking = shallowRef<(Confirmation & { answer: (confirmed: Confirmed | null) => void }) | null>(null);

/** Asks, and resolves to null if the answer is no. One question at a time: a new one cancels the last. */
export function confirm(question: Confirmation): Promise<Confirmed | null> {
  asking.value?.answer(null);
  return new Promise((resolve) => {
    asking.value = {
      ...question,
      answer(confirmed) {
        asking.value = null;
        resolve(confirmed);
      },
    };
  });
}
