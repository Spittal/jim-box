import { ref, watch, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

/** What a store link sells, best first. */
export type BuyLinkKind = 'album' | 'single' | 'track';

/** A store page selling a lossless copy. Only pages that exist are returned. */
export interface BuyLink {
  store: string;
  kind: BuyLinkKind;
  url: string;
}

/** The track to find in stores, with the album it's on. Mirrors `BuyQuery` in Rust. */
export interface BuyQuery {
  title: string;
  artist: string;
  album: string;
  albumArtist: string | null;
  mbReleaseGroupId: string | null;
}

export const BUY_KIND_LABELS: Record<BuyLinkKind, string> = {
  album: 'Album',
  single: 'Single',
  track: 'Track',
};

// Store lookups take a few requests each, so remember them for the session.
const cache = new Map<string, Promise<BuyLink[]>>();

function find(query: BuyQuery): Promise<BuyLink[]> {
  const key = JSON.stringify(query);
  let found = cache.get(key);
  if (!found) {
    found = invoke<BuyLink[]>('find_buy_links', { query });
    // Don't keep a failure; the next look tries again.
    found.catch(() => cache.delete(key));
    cache.set(key, found);
  }
  return found;
}

/** Store pages for the query, looked up whenever it changes. */
export function useBuyLinks(query: Ref<BuyQuery>) {
  const links = ref<BuyLink[]>([]);
  const loading = ref(false);
  // Only the latest lookup may set the results.
  let latest = 0;

  watch(
    () => JSON.stringify(query.value),
    async () => {
      const request = ++latest;
      loading.value = true;
      let found: BuyLink[] = [];
      try {
        found = await find(query.value);
      } catch {
        // Stores that can't be reached simply aren't listed.
      }
      if (request !== latest) return;
      links.value = found;
      loading.value = false;
    },
    { immediate: true },
  );

  return { links, loading };
}
