# Changelog

## 1.7.0

### New

- **Downloads window.** Downloads now have their own window, opened from the Downloads button at the foot of the window. It lists what's under way, what found no source or failed, and what's finished.
  - Pick a download to see its Soulseek sources, other files found, YouTube Music and places to buy it. A finished one can be shown in the library.
  - Select several to search again, cancel or remove them together. "Clear finished" tidies the list; the tracks stay in your library.
  - The button shows how many are downloading, and turns red when some need a look.
  - It opens when you start a download, once for a batch started together. Turn that off in Settings → Soulseek → Downloads.
  - A notice says when a download finishes while the window is closed.
- The selection's total length shows beside the number of tracks selected.
- A list opens scrolled to the track that's playing.

### Changed

- **Two searches.** ⌘F, or the magnifying glass beside Filter, searches the list being shown. ⇧⌘F and the field at the foot of the window search online, and say which playlist what you get goes into.
- Searching online shows that it's working, and can be cancelled with the Cancel button or Esc.
- Downloads no longer appear among the tracks in the list.
- Adding tracks a playlist already has says how many were left out.

### Fixed

- A file that can't be read to its end says where it stopped, instead of seeming to end early.
- The last moments of a track are no longer cut off when the next one starts.
- Keys no longer act on the list behind the USB and Downloads windows.

## 1.6.0

### New

- **Drag and drop.** Drag files and folders from Finder or Explorer onto the window to add them to the collection. Before, the window opened them itself.
  - Dropped on a playlist in the sidebar, they join that playlist too; dropped anywhere else, the playlist that's open.
  - The window says where a drop will put them while you drag.

## 1.5.0

### New

- **Export to USB.** Put playlists on a USB stick for CDJs and other players, from the new USB button in the bottom bar. The stick gets the audio, its analysis and both player databases, so it works on older and newer players.
  - A stick exported before is brought up to date: only what changed is copied, and tracks no longer chosen come off.
  - Shows what an export will copy and the room it needs before it starts.
  - Can convert files older players can't read to AIFF, WAV or MP3.
  - Can be stopped part way, leaving the stick as it was, and can eject the stick when it's done.

### Changed

- **One library, opened at launch.** Jim Box now opens the library rekordbox is set to use on this computer, without asking. The welcome screen, the library chooser and "Switch library" in Settings are gone.
- Settings, downloads and playlists saved for that library carry over.

### Fixed

- Cue names and downbeats in the track player.
