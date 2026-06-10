"""
audio-ripper — web app personale e privata per estrarre audio
da YouTube / SoundCloud (yt-dlp) e da link Spotify (solo metadati
via spotDL: l'audio viene sempre reperito da YouTube, mai dal
flusso DRM di Spotify).

Ascolta SOLO su 127.0.0.1 (vedi audio-ripper.service): l'accesso
esterno avviene tramite Tailscale o reverse-proxy con password.
"""

import asyncio
import re
import shutil
import sys
import time
import uuid
import zipfile
from pathlib import Path
from urllib.parse import urlparse

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse
from pydantic import BaseModel

BASE_DIR = Path(__file__).resolve().parent
DOWNLOADS_DIR = BASE_DIR / "downloads"
STATIC_DIR = BASE_DIR / "static"
COOKIES_FILE = BASE_DIR / "cookies.txt"

JOB_TTL_SECONDS = 30 * 60          # auto-pulizia: job e file > 30 minuti
CLEANUP_INTERVAL_SECONDS = 5 * 60

ALLOWED_HOSTS = {
    "youtube.com",
    "youtu.be",
    "music.youtube.com",
    "soundcloud.com",
    "on.soundcloud.com",
    "open.spotify.com",
}

ALLOWED_FORMATS = {"mp3", "wav"}


def _tool(name: str) -> str:
    """Risolve yt-dlp/spotdl dal venv corrente (il PATH di systemd
    non include venv/bin), con fallback sul PATH di sistema."""
    candidate = Path(sys.executable).parent / name
    return str(candidate) if candidate.exists() else name

# Stato dei job in memoria: nessun database.
# job_id -> {status, progress, message, title, filename, created_at, dir}
JOBS: dict[str, dict] = {}

app = FastAPI(title="audio-ripper", docs_url=None, redoc_url=None)


class DownloadRequest(BaseModel):
    url: str
    fmt: str


def _normalized_host(url: str) -> str | None:
    try:
        parsed = urlparse(url)
    except ValueError:
        return None
    if parsed.scheme not in ("http", "https"):
        return None
    host = (parsed.hostname or "").lower()
    for prefix in ("www.", "m."):
        if host.startswith(prefix):
            host = host[len(prefix):]
    return host or None


def _is_spotify(host: str) -> bool:
    return host == "open.spotify.com"


def _set(job_id: str, **kwargs) -> None:
    job = JOBS.get(job_id)
    if job is not None:
        job.update(kwargs)


def _audio_files(job_dir: Path, fmt: str) -> list[Path]:
    return sorted(p for p in job_dir.iterdir() if p.suffix.lower() == f".{fmt}")


def _finalize(job_id: str, job_dir: Path, fmt: str) -> None:
    """Individua il risultato: un file singolo, oppure uno zip se sono
    stati scaricati più brani (es. playlist Spotify)."""
    files = _audio_files(job_dir, fmt)
    if not files:
        _set(job_id, status="error", message="Nessun file audio prodotto.")
        return
    if len(files) == 1:
        result = files[0]
    else:
        result = job_dir / "playlist.zip"
        with zipfile.ZipFile(result, "w", zipfile.ZIP_STORED) as zf:
            for f in files:
                zf.write(f, f.name)
    title = JOBS[job_id].get("title") or files[0].stem
    _set(job_id, status="done", progress=100, message="Completato.",
         title=title, filename=result.name)


async def _run_ytdlp(job_id: str, url: str, fmt: str, job_dir: Path) -> None:
    cmd = [
        _tool("yt-dlp"),
        "-x",
        "--audio-format", fmt,
        "--no-playlist",
        "--newline",
        "-o", str(job_dir / "%(title)s.%(ext)s"),
    ]
    if fmt == "mp3":
        cmd += ["--audio-quality", "320K"]
    if COOKIES_FILE.exists():
        cmd += ["--cookies", str(COOKIES_FILE)]
    cmd.append(url)

    _set(job_id, status="running", message="Avvio di yt-dlp…")
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    progress_re = re.compile(r"\[download\]\s+(\d+(?:\.\d+)?)%")
    dest_re = re.compile(r"\[download\] Destination: (.+)")
    last_line = ""

    assert proc.stdout is not None
    while True:
        raw = await proc.stdout.readline()
        if not raw:
            break
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        last_line = line
        m = progress_re.search(line)
        if m:
            _set(job_id, progress=min(99, int(float(m.group(1)))),
                 message="Download in corso…")
            continue
        m = dest_re.search(line)
        if m:
            _set(job_id, title=Path(m.group(1)).stem)
            continue
        if "[ExtractAudio]" in line or "Extracting audio" in line:
            _set(job_id, progress=99, message="Conversione…")

    code = await proc.wait()
    if code != 0:
        _set(job_id, status="error",
             message=f"yt-dlp è uscito con codice {code}: {last_line[:200]}")
        return
    _finalize(job_id, job_dir, fmt)


async def _run_spotdl(job_id: str, url: str, fmt: str, job_dir: Path) -> None:
    """Link Spotify: spotDL legge SOLO i metadati dall'API ufficiale di
    Spotify (titolo, artista, album, copertina) e scarica l'audio
    equivalente da YouTube. Il flusso DRM di Spotify non viene mai
    toccato né decodificato."""
    cmd = [
        _tool("spotdl"), "download", url,
        "--format", fmt,
        "--output", str(job_dir / "{artists} - {title}.{output-ext}"),
    ]
    if fmt == "mp3":
        cmd += ["--bitrate", "320k"]
    if COOKIES_FILE.exists():
        cmd += ["--cookie-file", str(COOKIES_FILE)]

    _set(job_id, status="running",
         message="Lettura metadati da Spotify, audio da YouTube…")
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    found_re = re.compile(r"Found (\d+) songs?", re.IGNORECASE)
    done_re = re.compile(r'Downloaded "(.+?)"')
    total = 1
    done_count = 0
    last_line = ""

    assert proc.stdout is not None
    while True:
        raw = await proc.stdout.readline()
        if not raw:
            break
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        last_line = line
        m = found_re.search(line)
        if m:
            total = max(1, int(m.group(1)))
            _set(job_id, message=f"Trovati {total} brani, download da YouTube…")
            continue
        m = done_re.search(line)
        if m:
            done_count += 1
            _set(job_id,
                 progress=min(99, int(done_count / total * 100)),
                 title=m.group(1),
                 message=f"Scaricato {done_count}/{total}: {m.group(1)}")

    code = await proc.wait()
    files = _audio_files(job_dir, fmt)
    if code != 0 and not files:
        _set(job_id, status="error",
             message=f"spotDL è uscito con codice {code}: {last_line[:200]}")
        return
    _finalize(job_id, job_dir, fmt)


async def _run_job(job_id: str, url: str, fmt: str, spotify: bool) -> None:
    job_dir = Path(JOBS[job_id]["dir"])
    try:
        if spotify:
            await _run_spotdl(job_id, url, fmt, job_dir)
        else:
            await _run_ytdlp(job_id, url, fmt, job_dir)
    except FileNotFoundError as exc:
        _set(job_id, status="error",
             message=f"Eseguibile non trovato: {exc.filename or exc}. "
                     "Verifica che yt-dlp/spotdl/ffmpeg siano installati.")
    except Exception as exc:  # il job non deve mai uccidere il server
        _set(job_id, status="error", message=f"Errore interno: {exc}")


async def _cleanup_loop() -> None:
    while True:
        await asyncio.sleep(CLEANUP_INTERVAL_SECONDS)
        now = time.time()
        for job_id in list(JOBS):
            job = JOBS[job_id]
            if now - job["created_at"] > JOB_TTL_SECONDS:
                shutil.rmtree(job["dir"], ignore_errors=True)
                JOBS.pop(job_id, None)
        # orfani su disco (es. dopo un riavvio)
        if DOWNLOADS_DIR.exists():
            active = {Path(j["dir"]).name for j in JOBS.values()}
            for d in DOWNLOADS_DIR.iterdir():
                if d.is_dir() and d.name not in active and \
                        now - d.stat().st_mtime > JOB_TTL_SECONDS:
                    shutil.rmtree(d, ignore_errors=True)


@app.on_event("startup")
async def _startup() -> None:
    DOWNLOADS_DIR.mkdir(exist_ok=True)
    asyncio.create_task(_cleanup_loop())


@app.post("/api/download")
async def start_download(req: DownloadRequest):
    fmt = req.fmt.lower().strip()
    if fmt not in ALLOWED_FORMATS:
        raise HTTPException(400, "Formato non valido: usa 'mp3' o 'wav'.")
    host = _normalized_host(req.url.strip())
    if host is None or host not in ALLOWED_HOSTS:
        raise HTTPException(
            400,
            "URL non consentito. Sorgenti ammesse: YouTube, SoundCloud, "
            "Spotify (solo metadati).",
        )

    job_id = uuid.uuid4().hex
    job_dir = DOWNLOADS_DIR / job_id
    job_dir.mkdir(parents=True, exist_ok=True)
    JOBS[job_id] = {
        "status": "queued",
        "progress": 0,
        "message": "In coda…",
        "title": None,
        "filename": None,
        "created_at": time.time(),
        "dir": str(job_dir),
    }
    asyncio.create_task(_run_job(job_id, req.url.strip(), fmt,
                                 spotify=_is_spotify(host)))
    return {"job_id": job_id}


@app.get("/api/status/{job_id}")
async def job_status(job_id: str):
    job = JOBS.get(job_id)
    if job is None:
        raise HTTPException(404, "Job inesistente o scaduto.")
    return {
        "status": job["status"],
        "progress": job["progress"],
        "message": job["message"],
        "title": job["title"],
        "filename": job["filename"],
    }


@app.get("/api/file/{job_id}")
async def job_file(job_id: str):
    job = JOBS.get(job_id)
    if job is None or job["status"] != "done" or not job["filename"]:
        raise HTTPException(404, "File non disponibile o scaduto.")
    path = Path(job["dir"]) / job["filename"]
    if not path.exists():
        raise HTTPException(404, "File non disponibile o scaduto.")
    return FileResponse(path, filename=job["filename"],
                        media_type="application/octet-stream")


@app.get("/")
async def index():
    return FileResponse(STATIC_DIR / "index.html", media_type="text/html")
