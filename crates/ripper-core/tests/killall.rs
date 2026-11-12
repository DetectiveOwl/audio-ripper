//! `kill_all` acts on every registered child of the process, so this test lives
//! in its own file (its own process) to avoid killing jobs of other tests.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ripper_core::runner::{run_job, JobSpec, Status, Tools};
use ripper_core::{Format, Source};
use tokio::sync::Notify;

#[tokio::test]
async fn kill_all_stops_running_jobs() {
    let d = std::env::temp_dir().join(format!("ripper-test-killall-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    let y = d.join("yt-dlp");
    fs::write(&y, "#!/bin/sh\necho \"[download]  10.0% of 1MiB\"; sleep 30\n").unwrap();
    fs::set_permissions(&y, fs::Permissions::from_mode(0o755)).unwrap();
    let t = Tools { ytdlp: y, spotdl: d.join("none"), ffmpeg: d.join("ffmpeg"), deno: None, cookies: None };
    let spec = JobSpec { url: "https://youtu.be/x".into(), source: Source::Direct, format: Format::Mp3, out_dir: d.clone() };
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        ripper_core::children::kill_all();
    });
    let started = std::time::Instant::now();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let u2 = updates.clone();
    run_job(spec, &t, Arc::new(Notify::new()), move |u| u2.lock().unwrap().push(u)).await;
    let ups = updates.lock().unwrap().clone();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(ups.last().unwrap().status, Status::Error);
}
