# Audio Ripper

A **Windows** and **macOS** app that extracts audio from **YouTube**, **SoundCloud** and **Spotify**
links (single tracks or whole playlists) as **MP3 320 kbps** or **WAV**, with an analog
studio-rack interface.

Written in **Rust** with **Tauri 2**. There is no server: the app opens a single
fixed-size window (WebView2 on Windows, already included in Windows 11; the system
web view on macOS), runs yt-dlp, spotDL and
ffmpeg as child processes and saves files straight into the folder you choose. When you
close the window, nothing is left running.

## How it works

The Rust code is split into two crates. `crates/ripper-core` holds the logic and knows
nothing about the UI: it validates the URL against an allowlist of hosts, starts yt-dlp
(or spotDL for Spotify links), reads the output line by line to get progress and title,
and supports cancellation. `src-tauri` is the application shell: window, folder picker,
commands called by the interface and progress events sent back to it. The interface lives
in `ui/` (plain HTML, CSS and JS, no build step).

The window has a fixed size and no Windows title bar. The minimize and close buttons are
part of the rack panel, and dragging the panel moves the window.

For a clean exit, on Windows the app puts itself into a **Job Object** with
`KILL_ON_JOB_CLOSE` at startup: when the window is closed, even in the middle of a
download, Windows terminates all child processes (yt-dlp, ffmpeg, spotDL) on its own. On
macOS, where Job Objects do not exist, every child is started as the leader of its own
process group and the app kills the running groups when it exits (window closed, quit,
Cmd+Q). The **Cancel** button stops the running download and its descendants on both.

Files go to the destination folder (**Music** by default, changeable in the app and
remembered). There are no temporary copies and no timed cleanup.

## Using it

Paste one or more links (separated by spaces) and press **Extract audio**: they go into a
**queue** and are extracted one at a time. Links can be added while a download is running.
**Cancel** stops the current download and pauses the queue; **Extract audio** resumes it.

A YouTube playlist link (`youtube.com/playlist?list=...`) or a SoundCloud set goes into its
own subfolder named after the playlist, with numbered files. A video link that also carries
a `list=` parameter downloads only that video. Unavailable items in a playlist are skipped
and counted at the end.

MP3 files get **tags and the cover picture**. Under **File names** you can choose `Title`
or `Artist - Title` (the artist is added only when the site knows it). A file that is
already in the folder is not downloaded again. A download that shows no activity for
5 minutes is considered stuck and stopped.

Common errors (private, age-restricted, region-blocked or removed videos, network problems,
an outdated yt-dlp) are shown as a short explanation; hover the message for the original
text.

### Login cookies

Some videos need a logged-in account (age-restricted, members-only, or when YouTube asks
to "confirm you're not a bot"). Under **Login cookies** pick the browser you are logged in
with and yt-dlp reads its cookies; **Firefox** works best, Chrome and Edge can refuse while
they are open. You can also choose a `cookies.txt` file (Netscape format), which is copied
into the app's settings folder and is also used for Spotify links.

### Updates of yt-dlp and spotDL

YouTube changes often and an old yt-dlp stops working. Once a day (a few seconds after
launch, and after each new version of the app) Audio Ripper checks for new versions of
yt-dlp and spotDL and installs them in its data folder
(`%LOCALAPPDATA%\dev.audioripper.app\bin` on Windows,
`~/Library/Application Support/dev.audioripper.app/bin` on macOS), so no new release of
the app is needed. yt-dlp updates itself and verifies its own checksum; spotDL is downloaded
from its GitHub release, checked against the SHA-256 GitHub publishes (when there is one)
and tried before replacing the old copy. The footer shows the versions in use and has a
**Check for updates** button. Downloads wait while an update is running. The check uses
the system `curl`, which Windows 10/11 and macOS already include.

## Download

Get the latest version from the [**Releases** page](../../releases/latest) and pick the
file for your computer:

| Your computer | File |
|---|---|
| **Windows** (installer) | `AudioRipper-Windows-setup.exe` |
| **Windows** (no install, unzip and run) | `AudioRipper-Windows-portable.zip` |
| **Mac with an Apple chip** (M1 or newer) | `AudioRipper-macOS-apple-silicon.zip` |
| **Mac with an Intel processor** | `AudioRipper-macOS-intel.zip` |

Not sure which Mac you have? Open the Apple menu, then **About This Mac**: it says
"Chip Apple M1/M2/M3..." for Apple silicon, or "Processor Intel..." for Intel.

All versions already include yt-dlp, ffmpeg, spotDL and deno. The apps are not signed with
a paid certificate, so the first launch shows a security warning, explained below.

### Windows

Run the installer (per-user, no admin rights needed) or unzip the portable version and run
`audio-ripper.exe`.

**Windows SmartScreen warning.** On first launch Windows may show "Windows protected your
PC". Click **More info** and then **Run anyway**. For the portable version you can avoid it
by right-clicking the zip, choosing **Properties**, ticking **Unblock**, and only then
extracting it.

### macOS

Unzip, move **Audio Ripper.app** to Applications and open it.

**macOS Gatekeeper warning.** The app is not signed or notarized (that needs a paid Apple
Developer account), so macOS refuses to open it the first time. Either right-click the app,
choose **Open** and confirm, or go to **System Settings > Privacy & Security** and press
**Open Anyway**. If macOS says the app is damaged, run this once in Terminal:

```bash
xattr -cr "/Applications/Audio Ripper.app"
```

The macOS version has been tested on a real Mac.

### Publishing a release (maintainers)

Push a version tag and the **Release** workflow builds everything and publishes it:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The **Build Windows** and **Build macOS** workflows can also be run by hand from the
**Actions** tab; they produce the same files as downloadable artifacts (GitHub login required).

## Building locally (Windows)

You need Rust (`rustup`), Node.js and PowerShell.

```powershell
./scripts/fetch-binaries.ps1          # downloads yt-dlp, ffmpeg, spotDL, deno
npx @tauri-apps/cli@2 dev             # development window
npx @tauri-apps/cli@2 build           # installer in target/release/bundle/nsis
```

## Building locally (macOS)

You need Rust (`rustup`), Node.js and the Xcode command line tools.

```bash
./scripts/fetch-binaries.sh                      # downloads yt-dlp, ffmpeg, spotDL, deno
npx @tauri-apps/cli@2 dev                        # development window
npx @tauri-apps/cli@2 build --bundles app        # .app in target/release/bundle/macos
```

The core logic tests run on any system: `cargo test -p ripper-core`.

## Notes

- A `cookies.txt` chosen in the app is stored as
  `%APPDATA%\dev.audioripper.app\cookies.txt` (Windows) or
  `~/Library/Application Support/dev.audioripper.app/cookies.txt` (macOS). One put there by
  hand is still picked up as long as no other choice was made under Login cookies.
- The build scripts always download the latest yt-dlp, ffmpeg, spotDL and deno and check
  each against the SHA-256 its publisher provides (yt-dlp, deno and the Windows ffmpeg
  always; spotDL and the macOS ffmpeg when GitHub has a digest for them). The build stops
  on a mismatch.
- WAV output does not improve an already compressed source: it is only a lossless
  container from that point on.
