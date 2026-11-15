import { acceptHMRUpdate, defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { UsbConvert, UsbDevice, UsbEstimate, UsbProgress, UsbReport } from '@/types/usb';

/** Whole tracks of a stage, and how far through the one being copied. */
function fraction(p: UsbProgress): number | null {
  if (!p.total) return null;
  const within = p.fileTotal ? p.fileDone / p.fileTotal : 0;
  return Math.min(1, (p.done + within) / p.total);
}

/** USB sticks, and the export of playlists to one for players to read. */
export const useUsbStore = defineStore('usb', () => {
  const devices = ref<UsbDevice[]>([]);
  /** Looking for sticks, which reads each one. */
  const looking = ref(false);
  /** The stick being written, while one is. */
  const exporting = ref<string | null>(null);
  const progress = ref<UsbProgress | null>(null);
  /** Stop was asked for, and the export hasn't stopped yet. */
  const stopping = ref(false);
  /** How the last export went, until the next begins. */
  const report = ref<UsbReport | null>(null);
  const error = ref<string | null>(null);

  /** How far through its stage the export is, 0 to 100; null when the stage doesn't count. */
  const percent = computed(() => {
    const at = progress.value && fraction(progress.value);
    return at === null || at === undefined ? null : Math.round(at * 100);
  });

  let asked = 0;
  async function load() {
    const mine = ++asked;
    looking.value = true;
    try {
      const found = await invoke<UsbDevice[]>('usb_devices');
      if (mine === asked) devices.value = found;
    } catch (e) {
      if (mine === asked) error.value = String(e);
    } finally {
      if (mine === asked) looking.value = false;
    }
  }

  /** What an export would copy and need. Throws when the stick can't be read. */
  function estimate(path: string, playlists: string[], convert: UsbConvert): Promise<UsbEstimate> {
    return invoke<UsbEstimate>('usb_estimate', { path, playlists, convert });
  }

  /** Makes the stick hold these playlists. How it went is in `report`, or `error`. */
  async function run(path: string, playlists: string[], convert: UsbConvert, eject: boolean) {
    if (exporting.value) return;
    exporting.value = path;
    progress.value = null;
    report.value = null;
    error.value = null;
    stopping.value = false;
    try {
      report.value = await invoke<UsbReport>('usb_export', { path, playlists, convert, eject });
    } catch (e) {
      // Stopping it is not a failure.
      if (!stopping.value) error.value = String(e);
    } finally {
      exporting.value = null;
      progress.value = null;
      stopping.value = false;
      void load();
    }
  }

  function cancel() {
    if (!exporting.value) return;
    stopping.value = true;
    void invoke('cancel_usb_export');
  }

  async function eject(path: string) {
    error.value = null;
    try {
      await invoke('eject_usb', { path });
    } catch (e) {
      error.value = String(e);
    }
    await load();
  }

  return { devices, looking, exporting, progress, percent, stopping, report, error, load, estimate, run, cancel, eject };
});

// Swap in new actions on hot reload instead of keeping the old store.
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useUsbStore, import.meta.hot));
