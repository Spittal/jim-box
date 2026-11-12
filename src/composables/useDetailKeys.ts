/**
 * The keyboard's way round the collection's three panes — the playlists, the
 * track list and the track sidebar — and round the sidebar's entries.
 *
 * ⌘← and ⌘→ go from pane to pane (see CollectionView.vue); each is marked
 * `data-pane`, and the one the keyboard is in is the active one. In the
 * sidebar, Tab and ↑ ↓ go from one entry to the next and round again, coming
 * in at the entry marked `data-keys-first`. A row of buttons that belong
 * together — the stars, the colours, a field's suggestions — is one entry,
 * marked `data-keys-group`: it's stopped at on the one in use
 * (`data-keys-current`) or the first, and ← and → go along the row.
 */

const PANEL = '[data-track-detail]';
const LIST = '[data-track-list]';
const GROUP = '[data-keys-group]';
const FOCUSABLE = 'input, textarea, select, button';

/** Can take the keyboard, and can be seen to: not one shown only under the pointer. */
function usable(el: HTMLElement): boolean {
  if ((el as HTMLButtonElement).disabled || el.tabIndex < 0 || !el.getClientRects().length) return false;
  return el.checkVisibility?.({ opacityProperty: true, visibilityProperty: true }) ?? true;
}

function members(group: Element): HTMLElement[] {
  return [...group.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(usable);
}

/** The entries Tab and ↑ ↓ stop on, in order. */
function stops(panel: HTMLElement): HTMLElement[] {
  const found: HTMLElement[] = [];
  const groups = new Set<Element>();
  for (const el of panel.querySelectorAll<HTMLElement>(FOCUSABLE)) {
    if (!usable(el)) continue;
    const group = el.closest(GROUP);
    if (!group) found.push(el);
    else if (!groups.has(group)) {
      groups.add(group);
      const all = members(group);
      found.push(all.find((m) => m.hasAttribute('data-keys-current')) ?? all[0]);
    }
  }
  return found;
}

/**
 * Where the sidebar is come into: the entry marked `data-keys-first`, else the
 * first. The ones before it are reached by Shift-Tab or ↑, or by going round.
 */
function start(all: HTMLElement[]): number {
  return Math.max(0, all.findIndex((el) => el.hasAttribute('data-keys-first')));
}

function focus(el: HTMLElement) {
  el.focus();
  // As Tab does: what's typed next replaces what's there.
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) el.select();
  el.scrollIntoView({ block: 'nearest' });
}

/** Moves from an entry to the one after or before it, round the ends. */
export function stepEntry(panel: HTMLElement, from: HTMLElement, by: 1 | -1) {
  const all = stops(panel);
  if (!all.length) return;
  const group = from.closest(GROUP);
  const at = all.findIndex((el) => el === from || (!!group && el.closest(GROUP) === group));
  // From the sidebar itself: into the entry it's come into at, or its last going back.
  const to = at === -1 ? (by === 1 ? start(all) : all.length - 1) : (at + by + all.length) % all.length;
  focus(all[to]);
}

/** Moves along a row of buttons. False when `from` isn't in one. */
export function stepInGroup(from: HTMLElement, by: 1 | -1): boolean {
  const group = from.closest(GROUP);
  if (!group) return false;
  const all = members(group);
  const to = all[all.indexOf(from) + by];
  if (to) focus(to);
  return true;
}

/**
 * Puts the keyboard in the sidebar: on the sidebar itself, where the player's
 * and the list's keys still work; or in its first or last entry. False when
 * there's nowhere to put it.
 */
export function focusDetail(where: 'panel' | 'first' | 'last'): boolean {
  const panel = document.querySelector<HTMLElement>(PANEL);
  if (!panel) return false;
  if (where === 'panel') {
    panel.focus();
    return true;
  }
  const all = stops(panel);
  const to = all[where === 'first' ? start(all) : all.length - 1];
  if (!to) return false;
  focus(to);
  return true;
}

export type Pane = 'playlists' | 'list' | 'detail';
const PANES: Pane[] = ['playlists', 'list', 'detail'];

/** The pane something is in. Outside them all — the player, the toolbar — the list is the one keys go to. */
export function paneOf(el: Element | null): Pane {
  return (el?.closest<HTMLElement>('[data-pane]')?.dataset.pane as Pane | undefined) ?? 'list';
}

/** Gives a pane the keyboard: the pane itself, not anything in it. False when it isn't showing. */
export function focusPane(pane: Pane): boolean {
  const el = document.querySelector<HTMLElement>(
    pane === 'list' ? LIST : pane === 'detail' ? PANEL : '[data-pane="playlists"]',
  );
  el?.focus({ preventScroll: true });
  return !!el;
}

/** Goes to the pane to the left or right of the one `from` is in, if there is one. */
export function stepPane(from: Element | null, by: 1 | -1) {
  for (let at = PANES.indexOf(paneOf(from)) + by; at >= 0 && at < PANES.length; at += by) {
    if (focusPane(PANES[at])) return;
  }
}
