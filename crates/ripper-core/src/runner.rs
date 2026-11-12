//! Running a job: starts yt-dlp or spotDL, reads their output and reports
//! progress. One job at a time, no global state, no delayed cleanup: files go
//! straight into the folder chosen by the user.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::SystemTime;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Notify};

use crate::parse::{parse_spotdl_line, parse_ytdlp_line, title_from_path, SpotLine, YtLine};
use crate::{Format, Source};

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
    /// How many tracks were written (more than one for Spotify playlists).
    pub files: u32,
}

impl JobUpdate {
    fn running(progress: u8, message: impl Into<String>, title: Option<String>) -> Self {
        Self { status: Status::Running, progress, message: message.into(), title, file: None, files: 0 }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self { status: Status::Error, progress: 0, message: message.into(), title: None, file: None, files: 0 }
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
}

#[derive(Debug, Clone)]
pub struct JobSpec {
    pub url: String,
    pub source: Source,
    pub format: Format,
    pub out_dir: PathBuf,
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
        spec.out_dir.join("%(title)s.%(ext)s").to_string_lossy().into_owned(),
    ];
    if spec.format == Format::Mp3 {
        a.extend(["--audio-quality".into(), "320K".into()]);
    }
    if let Some(deno) = &tools.deno {
        a.push("--js-runtimes".into());
        a.push(format!("deno:{}", deno.to_string_lossy()));
    }
    if let Some(c) = &tools.cookies {
        a.push("--cookies".into());
        a.push(c.to_string_lossy().into_owned());
    }
    a.push(spec.url.clone());
    a
}

fn spotdl_args(spec: &JobSpec, tools: &Tools) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "download".into(),
        spec.url.clone(),
        "--format".into(),
        spec.format.ext().into(),
        "--output".into(),
        spec.out_dir.join("{artists} - {title}.{output-ext}").to_string_lossy().into_owned(),
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
    let mut last_error = String::new();
    let mut last_line = String::new();

    let outcome = run_process(cmd, cancel, |line| {
        if !line.trim().is_empty() {
            last_line = line.trim().to_string();
        }
        match parse_ytdlp_line(line) {
            YtLine::Progress(p) => {
                emit(JobUpdate::running(p.min(99), "Downloading…", title.clone()));
            }
            YtLine::Destination(d) => {
                if file.is_none() {
                    title = Some(title_from_path(&d));
                }
            }
            YtLine::Converting => emit(JobUpdate::running(99, "Converting…", title.clone())),
            YtLine::Finished(f) => {
                title = Some(title_from_path(&f));
                file = Some(f);
            }
            YtLine::Error(e) => last_error = e,
            YtLine::Other => {}
        }
    })
    .await?;

    match outcome {
        Outcome::Cancelled => emit(cancelled_update()),
        Outcome::Exit(Some(0)) => {
            let file = file.or_else(|| {
                newest_audio_file(&spec.out_dir, spec.format.ext(), started)
                    .map(|p| p.to_string_lossy().into_owned())
            });
            match file {
                Some(f) => emit(JobUpdate {
                    status: Status::Done,
                    progress: 100,
                    message: "Done.".into(),
                    title: title.or_else(|| Some(title_from_path(&f))),
                    file: Some(f),
                    files: 1,
                }),
                None => emit(JobUpdate::failed("No audio file was produced.")),
            }
        }
        Outcome::Exit(code) => {
            let detail = if last_error.is_empty() { last_line } else { last_error };
            emit(JobUpdate::failed(format!(
                "yt-dlp exited with code {}: {}",
                code.map_or("unknown".to_string(), |c| c.to_string()),
                truncate(&detail, 200)
            )));
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

    let outcome = run_process(cmd, cancel, |line| {
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
        Outcome::Exit(code) => {
            if code != Some(0) && done == 0 {
                emit(JobUpdate::failed(format!(
                    "spotDL exited with code {}: {}",
                    code.map_or("unknown".to_string(), |c| c.to_string()),
                    truncate(&last_line, 200)
                )));
                return Ok(());
            }
            let file = newest_audio_file(&spec.out_dir, spec.format.ext(), started)
                .map(|p| p.to_string_lossy().into_owned());
            if done == 0 && file.is_none() {
                emit(JobUpdate::failed("No audio file was produced."));
                return Ok(());
            }
            emit(JobUpdate {
                status: Status::Done,
                progress: 100,
                message: "Done.".into(),
                title,
                file,
                files: done.max(1),
            });
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
    }
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
        if modified + std::time::Duration::from_secs(2) < since {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

enum Outcome {
    Exit(Option<i32>),
    Cancelled,
}

fn base_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // yt-dlp and spotDL are Python programs: force UTF-8 on the pipes
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    #[cfg(windows)]
    {
        // no black console window for each child process
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Starts the process and passes every line (stdout and stderr together) to `on_line`.
async fn run_process(
    mut cmd: Command,
    cancel: &Notify,
    mut on_line: impl FnMut(&str),
) -> io::Result<Outcome> {
    let mut child = cmd.spawn()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, tx.clone()));
    }
    drop(tx);

    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some(l) => on_line(&l),
                None => break,
            },
            _ = &mut cancelled => {
                kill_tree(&mut child).await;
                return Ok(Outcome::Cancelled);
            }
        }
    }
    let status = child.wait().await?;
    Ok(Outcome::Exit(status.code()))
}

async fn pump(stream: impl AsyncRead + Unpin, tx: mpsc::UnboundedSender<String>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if tx.send(line).is_err() {
                    break;
                }
            }
        }
    }
}

/// Terminates the process and its descendants (ffmpeg launched by yt-dlp, etc.).
async fn kill_tree(child: &mut Child) {
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
    let _ = child.kill().await;
    let _ = child.wait().await;
}
