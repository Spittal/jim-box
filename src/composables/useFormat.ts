/** Display formatting helpers shared across views. */

import { isLossless, type AudioFormat } from '@/types/settings';

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatDuration(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.round(secs % 60);
  return `${m}:${String(s).padStart(2, '0')}`;
}

const FORMAT_LABELS: Record<string, string> = { vorbis: 'OGG' };

/** e.g. "FLAC 24/96", "FLAC 16/44.1" or "MP3 320k". */
export function formatQuality(
  format: string,
  bitDepth: number | null,
  sampleRate: number | null,
  bitRate: number | null = null,
): string {
  const parts = [FORMAT_LABELS[format] ?? format.toUpperCase()];
  if (!isLossless(format as AudioFormat)) {
    if (bitRate) parts.push(`${bitRate}k`);
    return parts.join(' ');
  }
  if (bitDepth && sampleRate) parts.push(`${bitDepth}/${+(sampleRate / 1000).toFixed(1)}`);
  else if (bitDepth) parts.push(`${bitDepth}-bit`);
  return parts.join(' ');
}

/** Soulseek paths are usually Windows-style. */
export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** "just now", "5 min ago", "3 h ago", "2 days ago". */
export function formatAgo(ms: number): string {
  const mins = Math.floor((Date.now() - ms) / 60_000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return `${days} ${days === 1 ? 'day' : 'days'} ago`;
}
