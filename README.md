# jim-box

Beets, plus finding lossless copies. Search for a track (or paste a public
Spotify playlist), and jim-box downloads the best lossless file (or,
failing that, the best lossy one) from Soulseek, tags it from MusicBrainz and
files it in your library the way [beets](https://beets.io) would. It can
also scan an existing library, match it album by album, and propose fixes for
you to review.

Built with Tauri 2 (Rust) and Vue 3. Soulseek access goes through a bundled
[slskd](https://github.com/slskd/slskd), run headless on localhost.

## Development

```sh
pnpm install
pnpm tauri:dev        # fetches slskd on first run, then starts the app
pnpm tauri:build      # release build

cd src-tauri
cargo test            # unit tests
cargo test -- --ignored --nocapture   # live tests against Deezer/MusicBrainz/Spotify
# live Soulseek search + ranking, against a running slskd
SLSKD_PORT=… SLSKD_API_KEY=… cargo test live_soulseek -- --ignored --nocapture
# live YouTube Music search, download and tagging (needs yt-dlp)
cargo test live_youtube -- --ignored --nocapture
```

The YouTube Music fallback uses yt-dlp from Homebrew (`brew install yt-dlp`),
which also installs the JavaScript runtime YouTube needs. It isn't bundled:
YouTube changes often enough that a bundled copy would soon stop working.

## Packaging (macOS)

```sh
pnpm tauri:build
```

This produces `jim-box.app` and a drag-to-install DMG in
`src-tauri/target/release/bundle/`. The build is ad-hoc signed, which is
enough to run it on this Mac. On another Mac, Gatekeeper will refuse it until
it's signed with a Developer ID and notarized: set `APPLE_SIGNING_IDENTITY`
(and `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` for notarization) before
building.

The build is for the Mac it runs on (Apple Silicon or Intel), as the bundled
slskd is.

### App icon

The icon is a head popping out of a cardboard box, drawn by `scripts/icon/`
(geometry, a layer renderer, and the asset builder) around the cut-out head
in `scripts/icon/head.png`:

```sh
python3 -m venv .venv-icon && .venv-icon/bin/pip install -r scripts/icon/requirements.txt
.venv-icon/bin/python scripts/icon/build.py
```

It writes `src-tauri/icons/AppIcon.icon`, an Icon Composer document (a
gradient fill under the box's back, the head and the box's front) that macOS
26 renders with its dark, tinted and clear variants, plus the pre-rendered
`icon.icns` and PNGs, and the DMG background. Tauri compiles `AppIcon.icon`
with Xcode 26's `actool`; without Xcode 26 the build logs an error about
`actool` and falls back to `icon.icns`, which is static.

To make the head from another photo (the photo itself isn't kept here):

```sh
swift scripts/icon/cutout.swift photo.png /tmp/mask.png > /tmp/landmarks.json
.venv-icon/bin/python scripts/icon/head.py photo.png /tmp/mask.png /tmp/landmarks.json
```

Closing the window keeps jim-box running, so downloads and Soulseek
sharing carry on. Click the Dock icon to bring it back, and quit with ⌘Q.

Logs, including slskd's output, go to `~/Library/Logs/com.jimbox.app/app.log`.

Dev builds keep the Soulseek password in `src-tauri/target/dev-secrets.json`
(release builds use the macOS Keychain), and run slskd from
`src-tauri/resources/slskd`.

## How it's organized

The backend mirrors beets' structure:

| Module (`src-tauri/src/`) | Role | Beets equivalent |
|---|---|---|
| `sources/` | API clients: Deezer (search), MusicBrainz (tags), Spotify (playlists), cover art, stores selling lossless downloads, YouTube Music (through yt-dlp). Return plain data. | metadata source plugins |
| `autotag/` | Identifying music: track, release and album matching with distances and confidence. | `beets.autotag` |
| `import/` | Library importer: scan, group into albums, propose, apply, move the folder. | `beets.importer` |
| `library/` | Files on disk: path templates (beets syntax), tagging, safe moves. | `beets.library` paths |
| `jobs/` | Finding lossless: Soulseek search, ranking, download, then tag and file. The manual YouTube Music fallback. | (the "plus") |
| `slskd/` | Runs and talks to the bundled slskd. | |
| `playlist.rs` | Public Spotify playlists, matched and cached. | |
| `commands/` | Tauri commands, grouped by domain; thin wrappers. | `beets.ui` commands |
| `state/` | Shared data types (settings, jobs, library items, tracks). | |

Conventions: errors are `AppError`; commands are thin and delegate to a
module; state lives in managed stores that emit events to the frontend; no
lock is held across `.await`.

The frontend (`src/`) uses Pinia stores for state and backend calls, views
per route, and small shared components (`TrackSummary`, `JobStatusBadge`,
`JobDetail`).

## Matching, in short

- **Search** shows Deezer's results (best ranking for popular music), then
  MusicBrainz versions Deezer doesn't have. You pick.
- **Tags** always come from MusicBrainz when it knows the recording, with the
  release chosen like beets' `match.preferred`: studio album over single,
  CD/digital over vinyl, earliest edition, the album you picked when present.
- **The library** opens as a folder browser: files are read locally (tags,
  artwork, quality) without looking anything up. Identifying is a separate
  step, for the whole library, a folder or one file.
- **Library scans** match folders as whole albums (title, artist, track titles,
  lengths and numbers), falling back to single tracks. Close matches are
  pre-selected; anything uncertain waits for your review.
- **Soulseek** results must be lossless, have the title in the file name, be
  the right version and within 5 seconds of the recording's length. Files
  with the right title that fail a check are listed separately, with the
  reason, to download by hand. When no lossless copy passes, the best lossy
  one (192 kbps or better) is used instead, unless that's turned off in
  settings or the download would replace a lossy file. When no peer answers,
  the job waits and searches again, for about half an hour.
- **Banned artist names**: the Soulseek server bans search phrases, many of
  them major-label artist names ("ariana grande"), and peers ignore searches
  containing one. When no peer answers any search naming the artist, Track
  Find searches without the artist's first word, then by title alone, and
  accepts only files whose path names the artist.
- **YouTube Music** is a manual fallback: a download Soulseek found nothing
  for offers "Get from YouTube Music". It picks the song whose title, artist,
  version and length match (preferring the ISRC's), downloads its AAC audio
  as is (256 kbps with Premium cookies from a browser, else 128 kbps), and
  tags and files it like any download. Being lossy, it's replaced when a
  later search finds a lossless copy.
- **Buy links** point only to store pages that exist: Bandcamp and Qobuz
  are searched, and other lossless stores (Beatport, 7digital…) come from
  MusicBrainz's purchase links once their pages check out. Each store gets
  one link, the album when it has it, else a single or the track.
- **Paths** follow beets' `paths`: separate templates for albums,
  compilations and tracks without an album, in beets syntax
  (`$albumartist/$album/$track $title`).
