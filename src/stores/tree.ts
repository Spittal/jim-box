import { defineStore } from 'pinia';
import { ref, watch } from 'vue';
import { useLibrariesStore } from '@/stores/libraries';

const KEY = 'tree.closed';

/**
 * The folders of playlists closed in each library's tree, kept between runs.
 * The master's are kept under `master`; another library's under its path,
 * as its id is only for this run, and without the `<id>~` its playlists'
 * ids start with, so a stick plugged in again comes back as it was left.
 */
export const useTreeStore = defineStore('tree', () => {
  const libraries = useLibrariesStore();

  /** Closed folders' ids, without their library's prefix, by where the library is. */
  const closed = ref<Record<string, string[]>>(load());

  function load(): Record<string, string[]> {
    try {
      const saved = JSON.parse(localStorage.getItem(KEY) ?? '{}');
      return saved && typeof saved === 'object' && !Array.isArray(saved) ? saved : {};
    } catch {
      return {};
    }
  }

  watch(closed, (all) => localStorage.setItem(KEY, JSON.stringify(all)), { deep: true });

  /** Where a library's closed folders are kept; null for one not open. */
  function keyOf(library: string | null): string | null {
    if (library === null) return 'master';
    const path = libraries.get(library)?.path;
    return path ? `path:${path}` : null;
  }

  const prefixOf = (library: string | null) => (library === null ? '' : `${library}~`);

  /** The folders closed in a library's tree, by their ids this run. */
  function closedIn(library: string | null): Set<string> {
    const key = keyOf(library);
    const prefix = prefixOf(library);
    return new Set(key ? (closed.value[key] ?? []).map((id) => prefix + id) : []);
  }

  /** Closes the folders `ids` of a library, or opens them. */
  function setClosed(library: string | null, ids: string[], close: boolean) {
    const key = keyOf(library);
    if (!key || !ids.length) return;
    const prefix = prefixOf(library);
    const bare = ids.map((id) => (id.startsWith(prefix) ? id.slice(prefix.length) : id));
    const now = new Set(closed.value[key] ?? []);
    for (const id of bare) {
      if (close) now.add(id);
      else now.delete(id);
    }
    if (now.size) closed.value[key] = [...now];
    else delete closed.value[key];
  }

  /** Forgets the folders of a library that are no longer in it, given every one that is. */
  function keepOnly(library: string | null, folderIds: string[]) {
    const key = keyOf(library);
    const was = key ? closed.value[key] : undefined;
    if (!key || !was) return;
    const prefix = prefixOf(library);
    const there = new Set(folderIds);
    const kept = was.filter((id) => there.has(prefix + id));
    if (kept.length === was.length) return;
    if (kept.length) closed.value[key] = kept;
    else delete closed.value[key];
  }

  return { closedIn, setClosed, keepOnly };
});
