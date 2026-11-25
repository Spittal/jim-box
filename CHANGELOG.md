# Changelog

## Unreleased

### New

- **Drag several playlists at once.** Pick playlists with ⌘- or Shift-click and drag any of them: they all go together, between your library and a stick or another library, or into a folder or a new place in the same library, keeping their order. A move within a library is undone in one step.

## 1.9.0

### New

- **Downloads try every quality in turn.** Lossless files on Soulseek come first, then lossy ones, then YouTube Music, until one works. Before, lossy files were only tried when no lossless one was found, and YouTube Music only by hand. YouTube Music is used when Fall back to lossy is on and yt-dlp is installed.
- **Every source, to pick by hand.** A download's page lists what it found, best first: lossless and lossy files on Soulseek, the YouTube Music songs it matched, and other files found. Pick any of them to try it now, even while the download is still searching or queued at a peer.
- Get from YouTube Music is offered on any download that hasn't finished, including one queued at a peer.

### Changed

- "Get lossless" is now "Download", everywhere.
- A lossy copy never replaces a file in your library: downloading a track again only replaces it with a lossless one.

### Fixed

- YouTube Music downloads no longer fail with "Requested format is not available" when signed in with a browser and yt-dlp can't get the audio that way. They're downloaded signed out instead, at 128 kbps.

## 1.8.0

### New

- **Other libraries beside yours.** Open library… at the foot of the sidebar opens a USB stick, or a folder with a rekordbox master.db in it (a backup, another computer's library, one kept on a drive). It gets its own part of the sidebar under yours, which is then labelled Master library, and you can open several.
  - Browse and play its tracks as you do your own, with their beat grids, cues and waveforms.
  - Drag tracks or playlists from one library to another. Whatever they come from wins: tracks you don't have are copied in, and ones you have take its beat grid, cues, rating and colour. Playlists with the same names in the same folders are updated to match.
  - Right-click a library's title to import or update your library from all of it, read it again, or close it. Right-click tracks or playlists to send them elsewhere.
  - A drive plugged in with a library on it is offered in the sidebar. A stick's library closes when it's pulled out, and the libraries you had open open again next time.
- **Changing a stick, then writing it.** Playlists made, renamed, deleted or filled on a stick, and your library's tracks and playlists dragged onto it, wait to be written, shown in italics. The USB button says how many changes are waiting; Apply to drive writes them all at once, both of the stick's databases alike. "Update Stick from Library" lines up the playlists it was exported with.
- New playlist and New folder are icons beside PLAYLISTS, in each library.

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
