import { computed } from 'vue';
import { useSettingsStore } from '@/stores/settings';
import type { KeyNotation } from '@/types/settings';

/** Pitch classes from C, spelled as rekordbox spells keys: flats, except F#. */
const NAMES = ['C', 'Db', 'D', 'Eb', 'E', 'F', 'F#', 'G', 'Ab', 'A', 'Bb', 'B'];
/** The Camelot wheel's minor keys, 1A to 12A, as pitch classes: fifths up from Ab minor. */
const CAMELOT_MINOR = [8, 3, 10, 5, 0, 7, 2, 9, 4, 11, 6, 1];
const LETTERS: Record<string, number> = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };

interface Key {
  /** The tonic, C being 0. */
  pitch: number;
  minor: boolean;
}

/** Reads a key however a library spells it: `Am`, `F#m`, `Eb`, `C#min`, or a Camelot code like `8A`. */
function parseKey(text: string): Key | null {
  const camelot = /^(\d{1,2})([ABab])$/.exec(text);
  if (camelot) {
    const number = Number(camelot[1]);
    if (number < 1 || number > 12) return null;
    const minor = camelot[2].toUpperCase() === 'A';
    const pitch = CAMELOT_MINOR[number - 1];
    // A major key is three semitones above its relative minor.
    return { pitch: minor ? pitch : (pitch + 3) % 12, minor };
  }
  const named = /^([A-Ga-g])([#♯b♭]*)\s*(m|min|minor|maj|major)?$/.exec(text);
  if (!named) return null;
  let pitch = LETTERS[named[1].toUpperCase()];
  for (const accidental of named[2]) pitch += accidental === '#' || accidental === '♯' ? 1 : -1;
  return { pitch: ((pitch % 12) + 12) % 12, minor: /^m(in|inor)?$/.test(named[3] ?? '') };
}

/** A key's number on the Camelot wheel, 0 to 11. */
const wheelNumber = (key: Key) => CAMELOT_MINOR.indexOf(key.minor ? key.pitch : (key.pitch + 9) % 12);

/**
 * Whether two keys mix: the same key, its neighbours either way round the
 * Camelot wheel, or its relative major or minor. False where either isn't a key.
 */
export function keysCompatible(a: string, b: string): boolean {
  const one = parseKey(a.trim());
  const other = parseKey(b.trim());
  if (!one || !other) return false;
  const apart = (wheelNumber(one) - wheelNumber(other) + 12) % 12;
  return one.minor === other.minor ? apart === 0 || apart === 1 || apart === 11 : apart === 0;
}

/** A key in the chosen notation. One that isn't recognised is shown as it's stored. */
export function formatKey(name: string, notation: KeyNotation): string {
  const text = name.trim();
  const key = parseKey(text);
  if (!key) return text;
  if (notation === 'camelot') {
    const relativeMinor = key.minor ? key.pitch : (key.pitch + 9) % 12;
    return `${CAMELOT_MINOR.indexOf(relativeMinor) + 1}${key.minor ? 'A' : 'B'}`;
  }
  return `${NAMES[key.pitch]}${key.minor ? 'm' : ''}`;
}

/** Every key, as rekordbox spells it, in the order the notation lists them: up from C, or round the Camelot wheel. */
export function allKeys(notation: KeyNotation): string[] {
  if (notation === 'camelot') {
    return CAMELOT_MINOR.flatMap((pitch) => [`${NAMES[pitch]}m`, NAMES[(pitch + 3) % 12]]);
  }
  return NAMES.flatMap((name) => [name, `${name}m`]);
}

/** Keys as the settings say to show them, whichever way the library stores each. */
export function useKeyNotation() {
  const store = useSettingsStore();
  const notation = computed<KeyNotation>(() => store.settings?.keyNotation ?? 'standard');
  return { notation, allKeys: () => allKeys(notation.value), formatKey: (name: string) => formatKey(name, notation.value) };
}
