/** How a track's beat grid was decided, in words. */

import type { GridChoice, GridIssue, GridReport, GridSource, GridState } from '@/types/collection';

const bpm = (value: number) => value.toFixed(2);

/** What the two beat detectors disagreed about, a sentence each. */
export function gridIssues(report: GridReport): string[] {
  const tempos = `rbxport ${bpm(report.signalBpm)}, Beat This ${bpm(report.modelBpm)} BPM`;
  const say: Record<GridIssue, string> = {
    unheard: 'Beat This heard too few beats to check this grid against.',
    varies: report.dynamic
      ? 'The tempo moves, so the grid follows it.'
      : 'The tempo moves, but the grid is at one tempo: following it is turned off in Settings.',
    tempo: `The two detectors found different beats (${tempos}), and the audio doesn't settle whose.`,
    uneven: 'Beat This’s beats were too uneven to follow, so the grid is at one tempo.',
    halfTime: `One detector counted two beats for the other’s one (${tempos}); the grid has Beat This’s count. Double or halve it if that’s the wrong one.`,
    offBeat: 'The detectors were half a beat apart; the grid is on Beat This’s beats.',
    downbeat: 'The detectors put the start of the bar on different beats; the grid has Beat This’s.',
    bar: 'Beat This wasn’t sure where the bar starts.',
  };
  return report.issues.map((issue) => say[issue]);
}

/**
 * Whose grid a track has, by name: the two beat detectors are rbxport's signal
 * analysis and the Beat This model, and the blend is what the analysis makes
 * of both.
 */
export const GRID_SOURCES: Record<GridSource, string> = {
  chosen: 'Blend',
  signal: 'rbxport',
  model: 'Beat This',
  hand: 'By hand',
};

/** What a grid to try is, for its button's tooltip. */
export function gridChoiceTitle(choice: GridChoice): string {
  const what: Record<GridSource, string> = {
    chosen: 'A blend of both detectors: Beat This’s pulse, placed on rbxport’s kick attacks',
    signal: 'rbxport’s grid alone, from the signal: one tempo, fitted to the kick attacks',
    model: 'Beat This’s grid alone, from the beat-tracking model: its beats, wherever the tempo goes',
    hand: '',
  };
  return `${what[choice.source]}. ${bpm(choice.bpm)} BPM${choice.dynamic ? ', following the tempo' : ''}.`;
}

/** Whether the grid a track has changes tempo along it. */
export function gridFollowsTempo(grid: GridState): boolean {
  const choice = grid.choices.find((c) => c.source === grid.using);
  return choice?.dynamic ?? (grid.using === 'chosen' && !!grid.report?.dynamic);
}

/** What the Grid column shows for a track: a tick once reviewed, else how sure the analysis was. */
export function gridLabel(grid: GridState): string {
  if (grid.reviewed) return '✓';
  return grid.report ? `${grid.report.confidence}%${gridFollowsTempo(grid) ? '~' : ''}` : '';
}

/** The same, at length, for a tooltip. */
export function gridSummary(grid: GridState): string {
  const lines: string[] = [];
  if (grid.reviewed) lines.push(grid.using === 'hand' ? 'Reviewed: you set this grid by hand.' : 'Reviewed: you marked this grid as right.');
  if (grid.report) {
    lines.push(
      grid.report.confidence === 100
        ? 'Both beat detectors found the same grid.'
        : `The two beat detectors, rbxport and Beat This, agreed ${grid.report.confidence}% on the grid.`,
      ...gridIssues(grid.report),
    );
  }
  if (!grid.reviewed && grid.choices.length > 1 && grid.using !== 'hand') lines.push(`The track has the ${GRID_SOURCES[grid.using]} grid.`);
  return lines.join('\n');
}

/** The colour it is shown in: green once reviewed, plain when sure, amber when worth a look, red when not to be trusted. */
export function gridClass(grid: GridState): string {
  if (grid.reviewed) return 'text-status-ok';
  if (!grid.report || grid.report.confidence >= 90) return 'text-text-muted';
  return grid.report.confidence >= 60 ? 'text-status-busy' : 'text-status-error';
}
