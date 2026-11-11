//! Test di integrazione con finti yt-dlp/spotDL (script shell): solo Unix.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ripper_core::runner::{run_job, JobSpec, JobUpdate, Status, Tools};
use ripper_core::{Format, Source};
use tokio::sync::Notify;

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn tools(dir: &Path, ytdlp: PathBuf, spotdl: PathBuf) -> Tools {
    Tools { ytdlp, spotdl, ffmpeg: dir.join("ffmpeg"), deno: None, cookies: None }
}

async fn collect(spec: JobSpec, tools: &Tools, cancel: Arc<Notify>) -> Vec<JobUpdate> {
    let updates = Arc::new(Mutex::new(Vec::new()));
    let u2 = updates.clone();
    run_job(spec, tools, cancel, move |u| u2.lock().unwrap().push(u)).await;
    let v = updates.lock().unwrap().clone();
    v
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ripper-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn ytdlp_success_reports_progress_and_file() {
    let d = tmp("ok");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "[download] Destination: {o}/Brano.webm"
echo "[download]  50.0% of 1MiB"
echo "[download] 100% of 1MiB"
echo "[ExtractAudio] Destination: {o}/Brano.mp3"
touch "{o}/Brano.mp3""#,
        o = out.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: out.clone() };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;

    assert!(ups.iter().any(|u| u.progress == 50 && u.status == Status::Running));
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.progress, 100);
    assert_eq!(last.title.as_deref(), Some("Brano"));
    assert!(last.file.as_deref().unwrap().ends_with("Brano.mp3"));
}

#[tokio::test]
async fn ytdlp_failure_reports_error_line() {
    let d = tmp("fail");
    let y = write_script(&d, "yt-dlp", r#"echo "ERROR: [youtube] x: Video unavailable" >&2; exit 1"#);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Wav, out_dir: d.clone() };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Error);
    assert!(last.message.contains("Video unavailable"), "{}", last.message);
}

#[tokio::test]
async fn missing_tool_is_reported() {
    let d = tmp("missing");
    let t = tools(&d, d.join("nope"), d.join("nope"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone() };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Error);
    assert!(last.message.contains("mancante"));
}

#[tokio::test]
async fn spotdl_playlist_counts_tracks() {
    let d = tmp("spot");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "Found 2 songs in Mix (Playlist)"
touch "{o}/A - Uno.mp3"
echo 'Downloaded "A - Uno": https://youtu.be/1'
touch "{o}/B - Due.mp3"
echo 'Downloaded "B - Due": https://youtu.be/2'"#,
        o = out.display()
    );
    let s = write_script(&d, "spotdl", &body);
    let t = tools(&d, d.join("none"), s);
    let spec = JobSpec { url: "https://open.spotify.com/playlist/x".into(), source: Source::Spotify, format: Format::Mp3, out_dir: out };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    assert!(ups.iter().any(|u| u.progress == 50));
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.files, 2);
    assert!(last.file.is_some());
}

#[tokio::test]
async fn cancel_kills_the_process() {
    let d = tmp("cancel");
    let y = write_script(&d, "yt-dlp", r#"echo "[download]  10.0% of 1MiB"; sleep 30"#);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone() };
    let cancel = Arc::new(Notify::new());
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.notify_one();
    });
    let started = std::time::Instant::now();
    let ups = collect(spec, &t, cancel).await;
    assert!(started.elapsed() < Duration::from_secs(10), "il processo non è stato terminato");
    assert_eq!(ups.last().unwrap().status, Status::Cancelled);
}
