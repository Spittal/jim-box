/** How a track's beat grid was decided, in words. */

import type { GridChoice, GridIssue, GridReport, GridSource, GridState } from '@/types/collection';

const bpm = (value: number) => value.toFixed(2);

/** What the two beat detectors disagreed about, a sentence each. */
export function gridIssues(report: GridReport): string[] {
  const tempos = `${bpm(report.signalBpm)} and ${bpm(report.modelBpm)} BPM`;
  const say: Record<GridIssue, string> = {
    unheard: 'The beat-tracking model heard too few beats to check this grid against.',
    varies: report.dynamic
      ? 'The tempo moves, so the grid follows it.'
      : 'The tempo moves, but the grid is at one tempo: following it is turned off in Settings.',
    tempo: `The two detectors found different beats (${tempos}), and the audio doesn't settle whose.`,
    uneven: 'The model’s beats were too uneven to follow, so the grid is at one tempo.',
    halfTime: `One detector counted two beats for the other’s one (${tempos}); the grid has the model’s count. Double or halve it if that’s the wrong one.`,
    offBeat: 'The detectors were half a beat apart; the grid is on the model’s beats.',
    downbeat: 'The detectors put the start of the bar on different beats; the grid has the model’s.',
    bar: 'The model wasn’t sure where the bar starts.',
  };
  return report.issues.map((issue) => say[issue]);
}

/** Whose grid a track has, as a few words. */
export const GRID_SOURCES: Record<GridSource, string> = {
  chosen: 'Chosen',
  signal: 'Signal',
  model: 'Model',
  hand: 'By hand',
};

/** What a grid to try is, for its button's tooltip. */
export function gridChoiceTitle(choice: GridChoice): string {
  const what: Record<GridSource, string> = {
    chosen: 'The grid the analysis decided on from both detectors',
    signal: 'The signal detector’s grid alone: one tempo, fitted to the kick attacks',
    model: 'The model’s grid alone: its beats, wherever the tempo goes',
    hand: '',
  };
  return `${what[choice.source]}. ${bpm(choice.bpm)} BPM${choice.dynamic ? ', following the tempo' : ''}.`;
}

/** What the Grid column shows for a track: a tick once reviewed, else how sure the analysis was. */
export function gridLabel(grid: GridState): string {
  if (grid.reviewed) return '✓';
  return grid.report ? `${grid.report.confidence}%${grid.report.dynamic && grid.using === 'chosen' ? '~' : ''}` : '';
}

/** The same, at length, for a tooltip. */
export function gridSummary(grid: GridState): string {
  const lines: string[] = [];
  if (grid.reviewed) lines.push(grid.using === 'hand' ? 'Reviewed: you set this grid by hand.' : 'Reviewed: you marked this grid as right.');
  if (grid.report) {
    lines.push(
      grid.report.confidence === 100
        ? 'Both beat detectors found the same grid.'
        : `The two beat detectors agreed ${grid.report.confidence}% on the grid the analysis chose.`,
      ...gridIssues(grid.report),
    );
  }
  if (!grid.reviewed && grid.using !== 'chosen') lines.push(`The track has the ${GRID_SOURCES[grid.using].toLowerCase()} grid instead.`);
  return lines.join('\n');
}

/** The colour it is shown in: green once reviewed, plain when sure, amber when worth a look, red when not to be trusted. */
export function gridClass(grid: GridState): string {
  if (grid.reviewed) return 'text-status-ok';
  if (!grid.report || grid.report.confidence >= 90) return 'text-text-muted';
  return grid.report.confidence >= 60 ? 'text-status-busy' : 'text-status-error';
}
