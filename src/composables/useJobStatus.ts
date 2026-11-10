import type { Job, JobStatus } from '@/types/job';
import { isLossless } from '@/types/settings';

const LABELS: Record<JobStatus, string> = {
  queued: 'Queued',
  identifying: 'Identifying',
  searching: 'Searching Soulseek',
  waiting: 'Waiting to search again',
  downloading: 'Downloading',
  tagging: 'Tagging',
  completed: 'Done',
  notFound: 'No source found',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

const COLORS: Record<JobStatus, string> = {
  queued: 'bg-status-idle',
  identifying: 'bg-status-busy',
  searching: 'bg-status-busy',
  waiting: 'bg-status-idle',
  downloading: 'bg-accent',
  tagging: 'bg-accent',
  completed: 'bg-status-ok',
  notFound: 'bg-status-error',
  failed: 'bg-status-error',
  cancelled: 'bg-status-idle',
};

export function statusLabel(job: Job): string {
  if (job.status === 'completed' && job.alreadyInLibrary) return 'Already in library';
  if (job.source === 'youtube') {
    if (job.status === 'searching') return 'Searching YouTube Music';
    if (job.status === 'completed') {
      return `Done (YouTube Music${job.youtube?.bitRate ? `, ${job.youtube.bitRate} kbps AAC` : ''})`;
    }
  }
  const used = job.activeCandidate === null ? null : job.candidates[job.activeCandidate];
  if (job.status === 'completed' && used && !isLossless(used.format)) return 'Done (lossy, no lossless found)';
  if (job.status === 'waiting' && job.retryAt) {
    const at = new Date(job.retryAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
    return `No answers, searching again at ${at}`;
  }
  if (job.status === 'downloading' && job.progress) {
    const { bytesTransferred, size, placeInQueue } = job.progress;
    if (bytesTransferred === 0 && placeInQueue) return `Queued at peer (#${placeInQueue})`;
    if (size > 0) return `Downloading ${Math.floor((bytesTransferred / size) * 100)}%`;
  }
  return LABELS[job.status];
}

export function statusColor(status: JobStatus): string {
  return COLORS[status];
}

export function progressPercent(job: Job): number | null {
  if (job.status !== 'downloading' || !job.progress || job.progress.size === 0) return null;
  return (job.progress.bytesTransferred / job.progress.size) * 100;
}
