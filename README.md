# audio-ripper

A self-hosted, single-user web app that extracts audio from **YouTube**, **SoundCloud**
and **Spotify links** as **MP3 320 kbps** or **WAV** — with an analog-studio-rack UI,
live progress, and a one-script cloud deployment.

> ⚠️ **Personal use only.** This tool is meant for downloading content you have the
> right to save (your own uploads, freely licensed material, authorized content).
> For Spotify links, **no audio is ever taken from Spotify**: spotDL reads track
> metadata (title, artist, album, cover art) from the official API and fetches the
> matching audio from YouTube. The DRM-protected Spotify stream is never touched.

![stack](https://img.shields.io/badge/FastAPI-async-009688) ![stack](https://img.shields.io/badge/yt--dlp%20%2B%20spotDL-extraction-ffb84d) ![stack](https://img.shields.io/badge/vanilla%20JS-no%20build%20step-f7df1e) ![stack](https://img.shields.io/badge/systemd%20%2B%20Tailscale-deploy-555)

## Architecture

```
Browser ──HTTPS──▶ Tailscale Funnel ──▶ 127.0.0.1:8000 (uvicorn, localhost-only)
                                            │
                              FastAPI async job manager (in-memory, no DB)
                                            │
                       ┌────────────────────┴────────────────────┐
                YouTube / SoundCloud                       Spotify links
                       │                                          │
                yt-dlp subprocess                          spotDL subprocess
              (stdout parsed line-by-line              (official API metadata →
               for live progress %)                     audio fetched from YouTube)
                       └────────────────────┬────────────────────┘
                                         ffmpeg
                                  MP3 320k / WAV output
```

Design choices worth noting:

- **The backend binds to `127.0.0.1` only.** Exposure happens through a tunnel
  (Tailscale Serve/Funnel) or a password-protected reverse proxy — the app itself
  is never directly reachable, and the VM needs no open HTTP ports.
- **Async job model without a database.** `POST /api/download` validates the URL's
  host against an allowlist (everything else → HTTP 400), spawns an asyncio task
  and returns a `job_id`. The frontend polls `GET /api/status/{id}` (~700 ms).
  Jobs live in a dict; files and state are garbage-collected after 30 minutes.
- **Real-time progress from a subprocess.** yt-dlp runs with `--newline` and its
  stdout is read line-by-line, mapping `[download] 42.3%` and the audio-extraction
  phase onto a 0–100 progress value and human-readable status messages.
- **Single-file frontend, zero build step.** Hand-written HTML/CSS/JS styled as an
  analog studio rack: machined corner screws, SVG fractal-noise grain, an animated
  24-bar EQ display while a job runs, amber LEDs on the format toggle.
- **Playlists degrade gracefully:** multiple resulting tracks are zipped server-side
  and served as one download.

## API

| Endpoint | Description |
|---|---|
| `POST /api/download` | `{url, fmt}` with `fmt ∈ {mp3, wav}` → `{job_id}`; 400 on non-allowlisted host |
| `GET /api/status/{job_id}` | `{status, progress, message, title, filename}` |
| `GET /api/file/{job_id}` | the resulting file (404 once expired) |
| `GET /` | the single-page UI |

## Stack

Python · FastAPI · Uvicorn · yt-dlp (+ Deno runtime for YouTube's JS challenges) ·
spotDL · ffmpeg · vanilla HTML/CSS/JS · systemd · Tailscale — running on an
always-free Oracle Cloud VM.

## Deployment

Designed for Ubuntu on Oracle Cloud Free Tier, but any Ubuntu box works:

```bash
scp -r audio-ripper ubuntu@<host>:~/
ssh ubuntu@<host>
cd ~/audio-ripper && bash setup.sh
```

`setup.sh` installs ffmpeg, Python, Deno and unzip, creates the venv, installs
pinned dependencies, registers the `audio-ripper` systemd service (auto-restart,
localhost-only) and installs Tailscale. Then either:

- **private access:** `sudo tailscale up && sudo tailscale serve --bg 8000`
  → reachable only from your own devices, or
- **public access:** front it with a reverse proxy (Caddy config with basic-auth
  included in the repo history) and/or `tailscale funnel`.

### Operational notes

- Some YouTube content requires a `cookies.txt` (Netscape format) in the project
  root — both yt-dlp and spotDL pick it up automatically. Keep it `chmod 600`,
  and never commit it (see `.gitignore`).
- yt-dlp and spotDL chase moving targets; update them monthly:
  `venv/bin/pip install -U yt-dlp spotdl && sudo systemctl restart audio-ripper`.
- WAV output does not improve an already-compressed source — it is just a
  lossless container from that point on. The UI says so, too.

## License / intent

Built as a personal tool and a portfolio piece. No accounts, no analytics, no
tracking, in-memory state only, auto-deleting files. Use it only on content you
are allowed to download, and respect the terms of the platforms involved.
