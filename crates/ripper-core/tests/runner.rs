//! Integration tests with fake yt-dlp/spotDL (shell scripts): Unix only.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ripper_core::runner::{run_job, JobSpec, JobUpdate, Status, Tools};
use ripper_core::{Format, NameStyle, Source};
use tokio::sync::Notify;

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn tools(dir: &Path, ytdlp: PathBuf, spotdl: PathBuf) -> Tools {
    Tools {
        ytdlp,
        spotdl,
        ffmpeg: dir.join("ffmpeg"),
        deno: None,
        cookies: None,
        cookies_browser: None,
        stall_timeout: Tools::DEFAULT_STALL_TIMEOUT,
    }
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
        r#"echo "[download] Destination: {o}/Track.webm"
echo "[download]  50.0% of 1MiB"
echo "[download] 100% of 1MiB"
echo "[ExtractAudio] Destination: {o}/Track.mp3"
touch "{o}/Track.mp3""#,
        o = out.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: out.clone(), name_style: NameStyle::Title };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;

    assert!(ups.iter().any(|u| u.progress == 50 && u.status == Status::Running));
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.progress, 100);
    assert_eq!(last.title.as_deref(), Some("Track"));
    assert!(last.file.as_deref().unwrap().ends_with("Track.mp3"));
}

#[tokio::test]
async fn ytdlp_failure_reports_error_line() {
    let d = tmp("fail");
    let y = write_script(&d, "yt-dlp", r#"echo "ERROR: [youtube] x: Video unavailable" >&2; exit 1"#);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Wav, out_dir: d.clone(), name_style: NameStyle::Title };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Error);
    assert!(last.message.contains("Video unavailable"), "{}", last.message);
}

#[tokio::test]
async fn missing_tool_is_reported() {
    let d = tmp("missing");
    let t = tools(&d, d.join("nope"), d.join("nope"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Error);
    assert!(last.message.contains("Missing"));
}

#[tokio::test]
async fn spotdl_playlist_counts_tracks() {
    let d = tmp("spot");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "Found 2 songs in Mix (Playlist)"
touch "{o}/A - One.mp3"
echo 'Downloaded "A - One": https://youtu.be/1'
touch "{o}/B - Two.mp3"
echo 'Downloaded "B - Two": https://youtu.be/2'"#,
        o = out.display()
    );
    let s = write_script(&d, "spotdl", &body);
    let t = tools(&d, d.join("none"), s);
    let spec = JobSpec { url: "https://open.spotify.com/playlist/x".into(), source: Source::Spotify, format: Format::Mp3, out_dir: out, name_style: NameStyle::Title };
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
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let cancel = Arc::new(Notify::new());
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.notify_one();
    });
    let started = std::time::Instant::now();
    let ups = collect(spec, &t, cancel).await;
    assert!(started.elapsed() < Duration::from_secs(10), "the process was not terminated");
    assert_eq!(ups.last().unwrap().status, Status::Cancelled);
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn cancel_also_kills_grandchildren() {
    let d = tmp("grandchild");
    let pidfile = d.join("pid");
    let body = format!(
        r#"sleep 30 &
echo $! > "{p}"
echo "[download]  10.0% of 1MiB"
wait"#,
        p = pidfile.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let cancel = Arc::new(Notify::new());
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        c2.notify_one();
    });
    let ups = collect(spec, &t, cancel).await;
    assert_eq!(ups.last().unwrap().status, Status::Cancelled);

    let pid: i32 = fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    for _ in 0..40 {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the grandchild process {pid} is still running");
}

#[tokio::test]
async fn friendly_error_for_known_failures() {
    let d = tmp("friendly");
    let y = write_script(&d, "yt-dlp", r#"echo "ERROR: [youtube] x: Private video. Sign in if you've been granted access" >&2; exit 1"#);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let last = collect(spec, &t, Arc::new(Notify::new())).await.pop().unwrap();
    assert_eq!(last.status, Status::Error);
    assert_eq!(last.message, "This video is private.");
    assert!(last.detail.as_deref().unwrap().contains("Private video"));
}

#[tokio::test]
async fn youtube_playlist_counts_items_and_overall_progress() {
    let d = tmp("ytplaylist");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "[download] Downloading item 1 of 2"
echo "[download] Destination: {o}/Mix/01 - A.webm"
echo "[download] 100% of 1MiB"
echo "[ExtractAudio] Destination: {o}/Mix/01 - A.mp3"
echo "[download] Downloading item 2 of 2"
echo "[download] Destination: {o}/Mix/02 - B.webm"
echo "[download]  50.0% of 1MiB"
echo "[ExtractAudio] Destination: {o}/Mix/02 - B.mp3""#,
        o = out.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://www.youtube.com/playlist?list=PL1".into(), source: Source::Direct, format: Format::Mp3, out_dir: out, name_style: NameStyle::Title };
    let ups = collect(spec, &t, Arc::new(Notify::new())).await;
    // item 2 at 50% is 75% overall
    assert!(ups.iter().any(|u| u.status == Status::Running && u.progress == 75 && u.message.starts_with("Item 2/2")), "{ups:?}");
    let last = ups.last().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.files, 2);
    assert_eq!(last.message, "Done: 2 tracks.");
}

#[tokio::test]
async fn playlist_with_failed_item_is_still_a_success() {
    let d = tmp("ytpartial");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "[download] Downloading item 1 of 2"
echo "ERROR: [youtube] a: Private video" >&2
echo "[download] Downloading item 2 of 2"
echo "[ExtractAudio] Destination: {o}/Mix/02 - B.mp3"
exit 1"#,
        o = out.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://www.youtube.com/playlist?list=PL1".into(), source: Source::Direct, format: Format::Mp3, out_dir: out, name_style: NameStyle::Title };
    let last = collect(spec, &t, Arc::new(Notify::new())).await.pop().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.files, 1);
    assert_eq!(last.message, "Done: 1 track, 1 skipped (errors).");
}

#[tokio::test]
async fn already_downloaded_file_is_reported_as_done() {
    let d = tmp("existing");
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let body = format!(
        r#"echo "[download] {o}/Track.mp3 has already been downloaded"
echo "[ExtractAudio] Not converting audio {o}/Track.mp3; file is already in target format mp3""#,
        o = out.display()
    );
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: out, name_style: NameStyle::Title };
    let last = collect(spec, &t, Arc::new(Notify::new())).await.pop().unwrap();
    assert_eq!(last.status, Status::Done);
    assert_eq!(last.files, 1, "the same file must not be counted twice");
}

#[tokio::test]
async fn silent_process_is_stopped_by_the_stall_timeout() {
    let d = tmp("stall");
    let y = write_script(&d, "yt-dlp", r#"echo "[download]  10.0% of 1MiB"; sleep 30"#);
    let mut t = tools(&d, y, d.join("none"));
    t.stall_timeout = Duration::from_millis(700);
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let started = std::time::Instant::now();
    let last = collect(spec, &t, Arc::new(Notify::new())).await.pop().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(last.status, Status::Error);
    assert!(last.message.contains("stuck"), "{}", last.message);
}

#[tokio::test]
async fn partial_progress_lines_keep_the_job_alive() {
    // a progress bar that redraws with \r and never prints \n is still activity
    let d = tmp("alive");
    let body = r#"for i in 1 2 3 4; do printf 'working %s\r' $i >&2; sleep 0.4; done
echo "[ExtractAudio] Destination: /tmp/ok.mp3""#;
    let y = write_script(&d, "yt-dlp", body);
    let mut t = tools(&d, y, d.join("none"));
    t.stall_timeout = Duration::from_millis(700);
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::Title };
    let last = collect(spec, &t, Arc::new(Notify::new())).await.pop().unwrap();
    assert_eq!(last.status, Status::Done, "{last:?}");
}

#[tokio::test]
async fn mp3_gets_tags_and_cover_and_browser_cookies_are_passed() {
    let d = tmp("args");
    let argsfile = d.join("args");
    let body = format!(r#"printf '%s\n' "$@" > "{}"
echo "[ExtractAudio] Destination: /tmp/x.mp3""#, argsfile.display());
    let y = write_script(&d, "yt-dlp", &body);
    let mut t = tools(&d, y, d.join("none"));
    t.cookies_browser = Some("firefox".into());
    t.cookies = Some(d.join("cookies.txt"));
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone(), name_style: NameStyle::ArtistTitle };
    collect(spec, &t, Arc::new(Notify::new())).await;
    let args = fs::read_to_string(&argsfile).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    for want in ["--embed-metadata", "--embed-thumbnail", "--cookies-from-browser", "firefox", "--no-playlist"] {
        assert!(lines.contains(&want), "missing {want} in {lines:?}");
    }
    assert!(!lines.contains(&"--cookies"), "the browser takes priority over cookies.txt");
    assert!(lines.iter().any(|l| l.contains("%(artist&{} - |)s%(title)s")));
}

#[tokio::test]
async fn wav_gets_no_cover_embedding() {
    let d = tmp("wavargs");
    let argsfile = d.join("args");
    let body = format!(r#"printf '%s\n' "$@" > "{}"
echo "[ExtractAudio] Destination: /tmp/x.wav""#, argsfile.display());
    let y = write_script(&d, "yt-dlp", &body);
    let t = tools(&d, y, d.join("none"));
    let spec = JobSpec { url: "https://www.youtube.com/playlist?list=PL1".into(), source: Source::Direct, format: Format::Wav, out_dir: d.clone(), name_style: NameStyle::Title };
    collect(spec, &t, Arc::new(Notify::new())).await;
    let args = fs::read_to_string(&argsfile).unwrap();
    assert!(!args.contains("--embed-thumbnail"));
    assert!(args.contains("%(playlist_title)s/%(playlist_index)02d - %(title)s.%(ext)s"), "{args}");
}
