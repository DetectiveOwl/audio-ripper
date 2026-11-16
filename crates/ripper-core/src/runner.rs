//! Running a job: starts yt-dlp or spotDL, reads their output and reports
//! progress. One job at a time, no global state, no delayed cleanup: files go
//! straight into the folder chosen by the user.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::errors::explain;
use crate::parse::{parse_spotdl_line, parse_ytdlp_line, title_from_path, SpotLine, YtLine};
use crate::{is_playlist_url, Format, NameStyle, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Done,
    Error,
    Cancelled,
}

/// Update sent to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct JobUpdate {
    pub status: Status,
    pub progress: u8,
    pub message: String,
    pub title: Option<String>,
    /// Path of the last file produced, if known.
    pub file: Option<String>,
    /// How many tracks were written (more than one for playlists).
    pub files: u32,
    /// The original error text, when `message` is a friendlier version of it.
    pub detail: Option<String>,
}

impl JobUpdate {
    fn running(progress: u8, message: impl Into<String>, title: Option<String>) -> Self {
        Self {
            status: Status::Running,
            progress,
            message: message.into(),
            title,
            file: None,
            files: 0,
            detail: None,
        }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self {
            status: Status::Error,
            progress: 0,
            message: message.into(),
            title: None,
            file: None,
            files: 0,
            detail: None,
        }
    }
    /// Error from a program's output: friendly text if we know the case, the raw
    /// text (with the program's name) otherwise. The raw text always goes in `detail`.
    fn failed_from(program: &str, code: Option<i32>, raw: &str) -> Self {
        let raw = raw.trim();
        let message = explain(raw).unwrap_or_else(|| {
            format!(
                "{program} exited with code {}: {}",
                code.map_or("unknown".to_string(), |c| c.to_string()),
                truncate(raw, 200)
            )
        });
        let mut u = Self::failed(message);
        if !raw.is_empty() {
            u.detail = Some(truncate(raw, 600));
        }
        u
    }
    fn done(message: impl Into<String>, title: Option<String>, file: Option<String>, files: u32) -> Self {
        Self { status: Status::Done, progress: 100, message: message.into(), title, file, files, detail: None }
    }
}

/// Paths of the external programs.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub spotdl: PathBuf,
    pub ffmpeg: PathBuf,
    /// JavaScript runtime for YouTube challenges (optional).
    pub deno: Option<PathBuf>,
    /// cookies.txt in Netscape format (optional).
    pub cookies: Option<PathBuf>,
    /// Browser to read login cookies from, yt-dlp only (optional, see `COOKIE_BROWSERS`).
    pub cookies_browser: Option<String>,
    /// A job that prints nothing for this long is considered stuck and stopped.
    pub stall_timeout: Duration,
}

impl Tools {
    pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);
}

#[derive(Debug, Clone)]
pub struct JobSpec {
    pub url: String,
    pub source: Source,
    pub format: Format,
    pub out_dir: PathBuf,
    pub name_style: NameStyle,
}

/// Runs the job to completion, emitting updates.
/// `cancel.notify_one()` stops the process (and its children).
pub async fn run_job(
    spec: JobSpec,
    tools: &Tools,
    cancel: Arc<Notify>,
    mut emit: impl FnMut(JobUpdate),
) {
    let started = SystemTime::now();
    emit(JobUpdate::running(0, "Starting…", None));
    let result = match spec.source {
        Source::Direct => run_ytdlp(&spec, tools, &cancel, &mut emit, started).await,
        Source::Spotify => run_spotdl(&spec, tools, &cancel, &mut emit, started).await,
    };
    if let Err(e) = result {
        let msg = if e.kind() == io::ErrorKind::NotFound {
            "Missing component (yt-dlp, ffmpeg or spotDL). Please reinstall the app.".to_string()
        } else {
            format!("Could not start the process: {e}")
        };
        emit(JobUpdate::failed(msg));
    }
}

/// yt-dlp output template: a subfolder per playlist, numbered files inside it.
fn out_template(spec: &JobSpec) -> String {
    let name = match spec.name_style {
        NameStyle::Title => "%(title)s",
        NameStyle::ArtistTitle => "%(artist&{} - |)s%(title)s",
    };
    let rel = if is_playlist_url(&spec.url) {
        format!("%(playlist_title)s/%(playlist_index)02d - {name}.%(ext)s")
    } else {
        format!("{name}.%(ext)s")
    };
    spec.out_dir.join(rel).to_string_lossy().into_owned()
}

fn ytdlp_args(spec: &JobSpec, tools: &Tools) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-x".into(),
        "--audio-format".into(),
        spec.format.ext().into(),
        "--no-playlist".into(),
        "--newline".into(),
        "--windows-filenames".into(),
        "--ffmpeg-location".into(),
        tools.ffmpeg.to_string_lossy().into_owned(),
        "-o".into(),
        out_template(spec),
    ];
    if spec.format == Format::Mp3 {
        // tags and cover art: only MP3 (ID3) supports them reliably
        a.extend([
            "--audio-quality".into(),
            "320K".into(),
            "--embed-metadata".into(),
            "--embed-thumbnail".into(),
            "--convert-thumbnails".into(),
            "jpg".into(),
        ]);
    }
    if let Some(deno) = &tools.deno {
        a.push("--js-runtimes".into());
        a.push(format!("deno:{}", deno.to_string_lossy()));
    }
    if let Some(browser) = &tools.cookies_browser {
        a.push("--cookies-from-browser".into());
        a.push(browser.clone());
    } else if let Some(c) = &tools.cookies {
        a.push("--cookies".into());
        a.push(c.to_string_lossy().into_owned());
    }
    a.push(spec.url.clone());
    a
}

fn spotdl_args(spec: &JobSpec, tools: &Tools) -> Vec<String> {
    let template = match spec.name_style {
        // spotDL always knows the artists
        NameStyle::ArtistTitle => "{artists} - {title}.{output-ext}",
        NameStyle::Title => "{title}.{output-ext}",
    };
    let mut a: Vec<String> = vec![
        "download".into(),
        spec.url.clone(),
        "--format".into(),
        spec.format.ext().into(),
        "--output".into(),
        spec.out_dir.join(template).to_string_lossy().into_owned(),
        "--ffmpeg".into(),
        tools.ffmpeg.to_string_lossy().into_owned(),
    ];
    if spec.format == Format::Mp3 {
        a.extend(["--bitrate".into(), "320k".into()]);
    }
    if let Some(c) = &tools.cookies {
        a.push("--cookie-file".into());
        a.push(c.to_string_lossy().into_owned());
    }
    a
}

/// Overall percentage when the job is item `n` of `total` and the item is at `p`%.
fn overall(item: (u32, u32), p: u8) -> u8 {
    let (n, total) = item;
    if total <= 1 {
        return p.min(99);
    }
    let done_before = n.saturating_sub(1).min(total) as f32;
    (((done_before * 100.0 + p as f32) / total as f32) as u8).min(99)
}

fn item_prefix(item: (u32, u32)) -> String {
    if item.1 > 1 { format!("Item {}/{}: ", item.0, item.1) } else { String::new() }
}

async fn run_ytdlp(
    spec: &JobSpec,
    tools: &Tools,
    cancel: &Notify,
    emit: &mut impl FnMut(JobUpdate),
    started: SystemTime,
) -> io::Result<()> {
    let mut cmd = base_command(&tools.ytdlp);
    cmd.args(ytdlp_args(spec, tools));

    let mut title: Option<String> = None;
    let mut file: Option<String> = None;
    let mut finished: HashSet<String> = HashSet::new();
    let mut item: (u32, u32) = (1, 1);
    let mut errors: u32 = 0;
    let mut last_error = String::new();
    let mut last_line = String::new();

    let outcome = run_process(cmd, cancel, tools.stall_timeout, |line| {
        if !line.trim().is_empty() {
            last_line = line.trim().to_string();
        }
        match parse_ytdlp_line(line) {
            YtLine::Progress(p) => {
                let msg = format!("{}Downloading…", item_prefix(item));
                emit(JobUpdate::running(overall(item, p), msg, title.clone()));
            }
            YtLine::Item(n, total) => {
                item = (n, total);
                file = None;
                emit(JobUpdate::running(overall(item, 0), item_prefix(item) + "Starting…", title.clone()));
            }
            YtLine::Destination(d) => {
                if file.is_none() {
                    title = Some(title_from_path(&d));
                }
            }
            YtLine::Converting => {
                let msg = format!("{}Converting…", item_prefix(item));
                emit(JobUpdate::running(overall(item, 99), msg, title.clone()));
            }
            YtLine::Tagging => {
                let msg = format!("{}Adding tags and cover…", item_prefix(item));
                emit(JobUpdate::running(overall(item, 99), msg, title.clone()));
            }
            YtLine::Existing(f) | YtLine::Finished(f) => {
                title = Some(title_from_path(&f));
                finished.insert(f.clone());
                file = Some(f);
            }
            YtLine::Error(e) => {
                errors += 1;
                last_error = e;
            }
            YtLine::Other => {}
        }
    })
    .await?;

    match outcome {
        Outcome::Cancelled => emit(cancelled_update()),
        Outcome::Stalled => emit(stalled_update(tools.stall_timeout)),
        Outcome::Exit(code) => {
            let playlist = item.1 > 1;
            // a single link that finished but left no "Finished" line: look in the folder
            let fallback = if finished.is_empty() && !playlist && code == Some(0) {
                newest_audio_file(&spec.out_dir, spec.format.ext(), started)
                    .map(|p| p.to_string_lossy().into_owned())
            } else {
                None
            };
            let files = finished.len() as u32 + fallback.is_some() as u32;
            let file = file.or(fallback);
            match (code, file) {
                (Some(0), Some(f)) => {
                    let msg = if files > 1 { format!("Done: {files} tracks.") } else { "Done.".into() };
                    let t = title.or_else(|| Some(title_from_path(&f)));
                    emit(JobUpdate::done(msg, t, Some(f), files.max(1)));
                }
                // some tracks failed (private or removed videos) or a tag step failed:
                // the audio is there, so it is a success with a note
                (_, Some(f)) if files > 0 => {
                    let msg = if playlist {
                        format!(
                            "Done: {files} {}, {} skipped (errors).",
                            if files == 1 { "track" } else { "tracks" },
                            errors.max(1)
                        )
                    } else {
                        "Done (the audio is saved, but tags or cover could not be added).".into()
                    };
                    let t = title.or_else(|| Some(title_from_path(&f)));
                    emit(JobUpdate::done(msg, t, Some(f), files));
                }
                (Some(0), None) => emit(JobUpdate::failed("No audio file was produced.")),
                (code, _) => {
                    let detail = if last_error.is_empty() { last_line } else { last_error };
                    emit(JobUpdate::failed_from("yt-dlp", code, &detail));
                }
            }
        }
    }
    Ok(())
}

/// Spotify links: spotDL reads only the metadata from the official API and
/// downloads the equivalent audio from YouTube.
async fn run_spotdl(
    spec: &JobSpec,
    tools: &Tools,
    cancel: &Notify,
    emit: &mut impl FnMut(JobUpdate),
    started: SystemTime,
) -> io::Result<()> {
    let mut cmd = base_command(&tools.spotdl);
    cmd.args(spotdl_args(spec, tools));
    emit(JobUpdate::running(0, "Reading metadata from Spotify, audio from YouTube…", None));

    let mut total: u32 = 1;
    let mut done: u32 = 0;
    let mut title: Option<String> = None;
    let mut last_line = String::new();

    let outcome = run_process(cmd, cancel, tools.stall_timeout, |line| {
        if !line.trim().is_empty() {
            last_line = line.trim().to_string();
        }
        match parse_spotdl_line(line) {
            SpotLine::Found(n) => {
                total = n.max(1);
                emit(JobUpdate::running(0, format!("Found {total} tracks, downloading from YouTube…"), None));
            }
            SpotLine::Downloaded(name) => {
                done += 1;
                let p = ((done as f32 / total as f32) * 100.0) as u8;
                emit(JobUpdate::running(p.min(99), format!("Downloaded {done}/{total}: {name}"), Some(name.clone())));
                title = Some(name);
            }
            SpotLine::Other => {}
        }
    })
    .await?;

    match outcome {
        Outcome::Cancelled => emit(cancelled_update()),
        Outcome::Stalled => emit(stalled_update(tools.stall_timeout)),
        Outcome::Exit(code) => {
            if code != Some(0) && done == 0 {
                emit(JobUpdate::failed_from("spotDL", code, &last_line));
                return Ok(());
            }
            let file = newest_audio_file(&spec.out_dir, spec.format.ext(), started)
                .map(|p| p.to_string_lossy().into_owned());
            if done == 0 && file.is_none() {
                emit(JobUpdate::failed("No audio file was produced."));
                return Ok(());
            }
            let n = done.max(1);
            let msg = if n > 1 { format!("Done: {n} tracks.") } else { "Done.".into() };
            emit(JobUpdate::done(msg, title, file, n));
        }
    }
    Ok(())
}

fn cancelled_update() -> JobUpdate {
    JobUpdate {
        status: Status::Cancelled,
        progress: 0,
        message: "Cancelled.".into(),
        title: None,
        file: None,
        files: 0,
        detail: None,
    }
}

fn stalled_update(after: Duration) -> JobUpdate {
    let mins = after.as_secs() / 60;
    let when = if mins >= 1 { format!("{mins} min") } else { format!("{} s", after.as_secs().max(1)) };
    JobUpdate::failed(format!("The download seems stuck (no activity for {when}) and was stopped."))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Newest audio file with the given extension, modified after `since`.
fn newest_audio_file(dir: &Path, ext: &str, since: SystemTime) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let matches_ext = path
            .extension()
            .map(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
            .unwrap_or(false);
        if !matches_ext {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        // small tolerance: some file systems round timestamps
        if modified + Duration::from_secs(2) < since {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

pub(crate) enum Outcome {
    Exit(Option<i32>),
    Cancelled,
    Stalled,
}

pub(crate) fn base_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // yt-dlp and spotDL are Python programs: force UTF-8 on the pipes
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    // macOS and Linux: the child leads its own process group, so the whole tree
    // (yt-dlp -> ffmpeg) can be killed at once
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        // no black console window for each child process
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

enum Chunk {
    Line(String),
    /// Bytes arrived, even if no line is complete yet (progress bars that
    /// redraw with `\r`): proves the program is alive.
    Activity,
}

/// Starts the process and passes every line (stdout and stderr together) to `on_line`.
/// Stops it if it stays silent for `stall` (`Outcome::Stalled`).
pub(crate) async fn run_process(
    mut cmd: Command,
    cancel: &Notify,
    stall: Duration,
    mut on_line: impl FnMut(&str),
) -> io::Result<Outcome> {
    let mut child = cmd.spawn()?;
    let _registered = child.id().map(crate::children::Registered::new);
    let (tx, mut rx) = mpsc::unbounded_channel::<Chunk>();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, tx.clone()));
    }
    drop(tx);

    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    let mut deadline = Instant::now() + stall;
    loop {
        tokio::select! {
            chunk = rx.recv() => match chunk {
                Some(Chunk::Line(l)) => { deadline = Instant::now() + stall; on_line(&l) }
                Some(Chunk::Activity) => deadline = Instant::now() + stall,
                None => break,
            },
            _ = &mut cancelled => {
                kill_tree(&mut child).await;
                return Ok(Outcome::Cancelled);
            }
            _ = tokio::time::sleep_until(deadline) => {
                kill_tree(&mut child).await;
                return Ok(Outcome::Stalled);
            }
        }
    }
    let status = child.wait().await?;
    Ok(Outcome::Exit(status.code()))
}

/// Reads a pipe and sends complete lines. `\n` and `\r` both end a line.
async fn pump(mut stream: impl AsyncRead + Unpin, tx: mpsc::UnboundedSender<Chunk>) {
    let mut buf = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(Chunk::Activity).is_err() {
                    return;
                }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !pending.is_empty() {
                            let line = String::from_utf8_lossy(&pending).trim_end().to_string();
                            pending.clear();
                            if tx.send(Chunk::Line(line)).is_err() {
                                return;
                            }
                        }
                    } else {
                        pending.push(b);
                    }
                }
            }
        }
    }
    if !pending.is_empty() {
        let _ = tx.send(Chunk::Line(String::from_utf8_lossy(&pending).trim_end().to_string()));
    }
}

/// Terminates the process and its descendants (ffmpeg launched by yt-dlp, etc.).
pub(crate) async fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut tk = Command::new("taskkill");
        tk.args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let _ = tk.status().await;
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        crate::children::kill_group(pid);
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
