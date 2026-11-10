/** What differs between macOS and Windows in the interface. */

export const isMac = /Mac/i.test(navigator.userAgent);
export const isWindows = /Windows/i.test(navigator.userAgent);

/** The key that adds to a selection and starts shortcuts: ⌘ on macOS, Ctrl elsewhere. */
export function commandKey(event: MouseEvent | KeyboardEvent): boolean {
  return isMac ? event.metaKey : event.ctrlKey;
}

/** A shortcut as the system writes it: `shortcut(',')` is "⌘," or "Ctrl+,". */
export function shortcut(key: string): string {
  return isMac ? `⌘${key}` : `Ctrl+${key}`;
}

/** The label of a button that shows a file in its folder. */
export const revealLabel = isMac ? 'Show in Finder' : 'Show in Explorer';

/** Where the system keeps saved passwords. */
export const passwordStore = isMac ? 'the macOS Keychain' : 'Windows Credential Manager';
