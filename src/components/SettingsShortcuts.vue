<script setup lang="ts">
import SettingsSection from '@/components/SettingsSection.vue';
import { isMac } from '@/platform';

/**
 * The keyboard shortcuts, to read: they can't be changed. Kept by hand, so a
 * shortcut added elsewhere is to be added here too.
 */

const cmd = isMac ? '⌘' : 'Ctrl';
const shift = isMac ? '⇧' : 'Shift';
const del = isMac ? '⌫' : 'Delete';
const enter = isMac ? '↩' : 'Enter';

/** What a shortcut does, and the ways of pressing it: each a run of keys held together. */
type Line = { does: string; keys: string[][] };

const groups: { title: string; note?: string; lines: Line[] }[] = [
  {
    title: 'Reviewing',
    lines: [
      { does: 'Mark the grid reviewed and load the next track', keys: [[cmd, 'S']] },
      { does: 'Load the picked track into the player, and go to its sidebar', keys: [[enter]] },
      { does: 'Analyse the track again: opens the form, then ' + enter + ' goes ahead', keys: [[cmd, 'L']] },
      { does: 'Identify the track on MusicBrainz', keys: [[cmd, 'I']] },
      { does: 'Download the track again, or a lossless copy', keys: [[cmd, 'R']] },
      { does: 'Remove the track; from a playlist, only from that', keys: [[del]] },
      { does: 'Delete the track and its file', keys: [[cmd, del]] },
    ],
  },
  {
    title: 'Panes',
    note: 'A line along the top shows which has the keyboard.',
    lines: [
      { does: 'Between the playlists, the tracks and the sidebar', keys: [[cmd, '←'], [cmd, '→']] },
      { does: 'Pick the playlist or track above or below', keys: [['↑'], ['↓']] },
      { does: 'Close or open a folder of playlists', keys: [['←'], ['→']] },
    ],
  },
  {
    title: 'Track list',
    lines: [
      { does: 'Pick more tracks', keys: [[shift, '↑'], [shift, '↓']] },
      { does: 'First or last track', keys: [[cmd, '↑'], [cmd, '↓']] },
      { does: 'A screenful up or down', keys: [['Page Up'], ['Page Down']] },
      { does: 'Pick every track', keys: [[cmd, 'A']] },
      { does: 'Pick nothing', keys: [['Esc']] },
    ],
  },
  {
    title: 'Player',
    note: 'Not while typing in a text field: press Esc to leave it first. Space plays and pauses from anywhere else, buttons included.',
    lines: [
      { does: 'Play or pause', keys: [['Space']] },
      { does: 'Back or forward a beat', keys: [[','], ['.']] },
      { does: 'Back or forward 8 bars', keys: [[cmd, ','], [cmd, '.']] },
      { does: 'Back or forward 16 bars', keys: [[shift, ','], [shift, '.']] },
      { does: 'Go to a hot cue, or set it where there is none', keys: [['1'], ['…'], ['8']] },
      { does: 'Delete a hot cue', keys: [[shift, '1'], ['…'], [shift, '8']] },
      { does: 'Set a memory cue', keys: [['M'], [cmd, 'M']] },
      { does: 'Set the downbeat: the beat at the playhead starts its bar', keys: [[cmd, 'B']] },
    ],
  },
  {
    title: 'Track sidebar',
    lines: [
      { does: 'Rate the track one to five stars; its rating again clears it', keys: [[cmd, '1'], ['…'], [cmd, '5']] },
      { does: 'The next entry, from Genre on; or the one before', keys: [['Tab'], [shift, 'Tab']] },
      { does: 'The entry above or below', keys: [['↑'], ['↓']] },
      { does: 'Along the genres, playlists, stars or colours', keys: [['←'], ['→']] },
      { does: 'Choose the one the keyboard is on, or open the Key menu', keys: [[enter]] },
      { does: 'Through what a field suggests, once typing shows it', keys: [['↑'], ['↓']] },
      { does: 'Offer the playlists to add the track to', keys: [[enter]] },
      { does: 'Take the track out of the playlist the keyboard is on', keys: [[del]] },
      { does: 'Leave the entry; again to pick nothing', keys: [['Esc']] },
    ],
  },
  {
    title: 'Window',
    lines: [
      { does: 'Find in the collection', keys: [[cmd, 'F']] },
      { does: 'Find online', keys: [[cmd, shift, 'F']] },
      { does: 'Settings', keys: [[cmd, ',']] },
      { does: 'These shortcuts', keys: [[cmd, '/'], ['?']] },
      { does: 'Close settings or a dialog', keys: [['Esc']] },
    ],
  },
];
</script>

<template>
  <SettingsSection title="Keyboard shortcuts">
    <div v-for="group in groups" :key="group.title" class="flex flex-col gap-1.5">
      <h3 class="section-label">{{ group.title }}</h3>
      <p v-if="group.note" class="text-[11px] text-text-muted">{{ group.note }}</p>
      <dl class="flex flex-col">
        <div
          v-for="line in group.lines"
          :key="line.does"
          class="flex items-center justify-between gap-4 border-b border-border/50 py-1.5 text-xs last:border-b-0"
        >
          <dt class="text-text-secondary">{{ line.does }}</dt>
          <dd class="flex shrink-0 items-center gap-2">
            <span v-for="(keys, at) in line.keys" :key="at" class="flex items-center gap-0.5">
              <template v-for="key in keys" :key="key">
                <span v-if="key === '…'" class="text-text-muted">…</span>
                <kbd
                  v-else
                  class="min-w-5 rounded border border-border bg-surface-2 px-1.5 py-0.5 text-center font-sans text-[11px] text-text-primary"
                >{{ key }}</kbd>
              </template>
            </span>
          </dd>
        </div>
      </dl>
    </div>
  </SettingsSection>
</template>
