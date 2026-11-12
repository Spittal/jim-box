/**
 * The keyboard's way round the track sidebar (CollectionTrackDetail.vue).
 *
 * Tab goes between the track list and the sidebar, coming in at the entry
 * marked `data-keys-first`. There, ↑ and ↓ go from one entry to the next and
 * round again. A row of buttons that belong together — the stars, the
 * colours, a field's suggestions — is one entry, marked `data-keys-group`:
 * ↑ and ↓ stop on the one in use (`data-keys-current`) or the first, and ← and
 * → go along the row.
 */

const PANEL = '[data-track-detail]';
const LIST = '[data-track-list]';
const GROUP = '[data-keys-group]';
const FOCUSABLE = 'input, textarea, select, button';

function usable(el: HTMLElement): boolean {
  return !(el as HTMLButtonElement).disabled && el.tabIndex >= 0 && el.getClientRects().length > 0;
}

function members(group: Element): HTMLElement[] {
  return [...group.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(usable);
}

/** The entries ↑ and ↓ stop on, in order. */
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
 * Where Tab comes in: the entry marked `data-keys-first`, else the first. The
 * ones before it are reached by ↑, or by going round.
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
  // From the sidebar itself: into the entry Tab comes in at, or its last going up.
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

/** Gives the keyboard back to the track list: ↑ and ↓ pick tracks again. */
export function focusList() {
  const list = document.querySelector<HTMLElement>(LIST);
  if (list) list.focus({ preventScroll: true });
  else if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
}
