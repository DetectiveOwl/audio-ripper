# Audio Ripper

A **Windows** and **macOS** app that extracts audio from **YouTube**, **SoundCloud** and **Spotify**
links as **MP3 320 kbps** or **WAV**, with an analog studio-rack interface.

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

The macOS version is built by CI and has not been tested on a real Mac by the author yet:
issues are welcome.

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

- Some YouTube content needs a `cookies.txt` in Netscape format. Put it in
  `%APPDATA%\dev.audioripper.app\cookies.txt` (Windows) or
  `~/Library/Application Support/dev.audioripper.app/cookies.txt` (macOS) and it will be
  used by yt-dlp and spotDL.
- yt-dlp and spotDL chase moving targets: to update them, just run the build again, which
  always downloads the latest versions.
- WAV output does not improve an already compressed source: it is only a lossless
  container from that point on.
