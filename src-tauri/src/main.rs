// On Windows, in release builds, no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ripper_core::runner::{run_job, JobSpec, Tools};
use ripper_core::update::{UpdateReport, Updater};
use ripper_core::{validate_url, Format, NameStyle, COOKIE_BROWSERS};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Notify;

/// What the app is doing: at most one thing at a time, so that a tool is never
/// replaced by an update while a download is using it.
#[derive(Default)]
enum Busy {
    #[default]
    Idle,
    Job(Arc<Notify>),
    Updating,
}

#[derive(Default)]
struct AppState {
    busy: Mutex<Busy>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct Settings {
    out_dir: Option<String>,
    /// "mp3" or "wav".
    format: Option<String>,
    /// "title" or "artist_title".
    name_style: Option<String>,
    /// "none", "file" or a browser name; unset means: use cookies.txt if present.
    cookies: Option<String>,
}

/// Sent to the interface while the tools are being checked, and when done.
#[derive(Serialize, Clone)]
struct ToolsStatus {
    /// "checking" or "idle".
    state: &'static str,
    message: String,
    ytdlp: Option<String>,
    spotdl: Option<String>,
    updated: Vec<String>,
    errors: Vec<String>,
}

impl ToolsStatus {
    fn from_report(state: &'static str, message: &str, r: UpdateReport) -> Self {
        Self {
            state,
            message: message.to_string(),
            ytdlp: r.ytdlp_version,
            spotdl: r.spotdl_version,
            updated: r.updated,
            errors: r.errors,
        }
    }
}

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn load_settings(app: &AppHandle) -> Settings {
    config_dir(app)
        .ok()
        .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let dir = config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), json).map_err(|e| e.to_string())
}

/// Folder chosen by the user, otherwise Music, otherwise Downloads.
fn resolve_out_dir(app: &AppHandle) -> PathBuf {
    if let Some(dir) = load_settings(app).out_dir {
        return PathBuf::from(dir);
    }
    app.path()
        .audio_dir()
        .or_else(|_| app.path().download_dir())
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "App folder not found.".to_string())
}

/// Finds yt-dlp and spotDL: updated copies (see `ripper_core::update`) win over
/// the bundled ones next to the executable.
fn make_updater(app: &AppHandle) -> Result<Updater, String> {
    let data = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("bin");
    Ok(Updater::new(
        exe_dir()?,
        data,
        &app.package_info().version.to_string(),
    ))
}

/// Login cookies for yt-dlp: (cookies.txt, browser). The browser wins.
fn cookie_setup(app: &AppHandle, settings: &Settings) -> (Option<PathBuf>, Option<String>) {
    let file = config_dir(app)
        .ok()
        .map(|d| d.join("cookies.txt"))
        .filter(|p| p.exists());
    match settings.cookies.as_deref() {
        Some("none") => (None, None),
        Some("file") => (file, None),
        Some(b) if COOKIE_BROWSERS.contains(&b) => (None, Some(b.to_string())),
        // never chosen: a cookies.txt put there by hand keeps working
        _ => (file, None),
    }
}

/// The external tools: the bundled ones sit next to the executable (Tauri sidecars).
fn resolve_tools(app: &AppHandle) -> Result<Tools, String> {
    let dir = exe_dir()?;
    let name = |n: &str| {
        dir.join(if cfg!(windows) {
            format!("{n}.exe")
        } else {
            n.to_string()
        })
    };
    let deno = name("deno");
    let updater = make_updater(app)?;
    let (cookies, cookies_browser) = cookie_setup(app, &load_settings(app));
    Ok(Tools {
        ytdlp: updater.resolve("yt-dlp"),
        spotdl: updater.resolve("spotdl"),
        ffmpeg: name("ffmpeg"),
        deno: deno.exists().then_some(deno),
        cookies,
        cookies_browser,
        stall_timeout: Tools::DEFAULT_STALL_TIMEOUT,
    })
}

#[tauri::command]
fn get_out_dir(app: AppHandle) -> String {
    resolve_out_dir(&app).to_string_lossy().into_owned()
}

#[tauri::command]
async fn pick_out_dir(app: AppHandle) -> Result<Option<String>, String> {
    let app2 = app.clone();
    let picked =
        tauri::async_runtime::spawn_blocking(move || app2.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let dir = path.to_string_lossy().into_owned();
    let mut settings = load_settings(&app);
    settings.out_dir = Some(dir.clone());
    save_settings(&app, &settings)?;
    Ok(Some(dir))
}

/// What the interface needs to draw its controls.
#[derive(Serialize)]
struct UiSettings {
    format: String,
    name_style: String,
    cookies: String,
}

#[tauri::command]
fn get_settings(app: AppHandle) -> UiSettings {
    let s = load_settings(&app);
    let has_file = config_dir(&app)
        .map(|d| d.join("cookies.txt").exists())
        .unwrap_or(false);
    UiSettings {
        format: s
            .format
            .filter(|f| Format::parse(f).is_some())
            .unwrap_or_else(|| "mp3".into()),
        name_style: s
            .name_style
            .filter(|n| NameStyle::parse(n).is_some())
            .unwrap_or_else(|| "title".into()),
        cookies: s.cookies.unwrap_or_else(|| {
            if has_file {
                "file".into()
            } else {
                "none".into()
            }
        }),
    }
}

/// Saves one remembered choice after checking it.
#[tauri::command]
fn set_option(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let mut s = load_settings(&app);
    match key.as_str() {
        "format" if Format::parse(&value).is_some() => s.format = Some(value),
        "name_style" if NameStyle::parse(&value).is_some() => s.name_style = Some(value),
        "cookies"
            if value == "none" || value == "file" || COOKIE_BROWSERS.contains(&value.as_str()) =>
        {
            s.cookies = Some(value)
        }
        _ => return Err(format!("Invalid option {key}.")),
    }
    save_settings(&app, &s)
}

/// Lets the user pick a cookies.txt (Netscape format) and keeps a copy of it.
/// Returns true if one was imported.
#[tauri::command]
async fn import_cookies(app: AppHandle) -> Result<bool, String> {
    let app2 = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .file()
            .add_filter("cookies.txt", &["txt"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(false);
    };
    let src = picked.into_path().map_err(|e| e.to_string())?;
    let dir = config_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::copy(&src, dir.join("cookies.txt"))
        .map_err(|e| format!("Could not copy the file: {e}"))?;
    let mut s = load_settings(&app);
    s.cookies = Some("file".into());
    save_settings(&app, &s)?;
    Ok(true)
}

#[tauri::command]
async fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    fmt: String,
) -> Result<(), String> {
    let format = Format::parse(&fmt).ok_or("Invalid format: use MP3 or WAV.")?;
    let (url, source) = validate_url(&url)?;
    let settings = load_settings(&app);
    let name_style = settings
        .name_style
        .as_deref()
        .and_then(NameStyle::parse)
        .unwrap_or_default();
    let out_dir = resolve_out_dir(&app);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("Destination folder not usable: {e}"))?;
    let tools = resolve_tools(&app)?;

    let cancel = Arc::new(Notify::new());
    {
        let mut busy = state.busy.lock().unwrap();
        match *busy {
            Busy::Idle => *busy = Busy::Job(cancel.clone()),
            Busy::Job(_) => return Err("An extraction is already in progress.".into()),
            Busy::Updating => {
                return Err("The components are being updated: try again in a moment.".into())
            }
        }
    }

    let spec = JobSpec {
        url,
        source,
        format,
        out_dir,
        name_style,
    };
    tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        run_job(spec, &tools, cancel, move |update| {
            let _ = emitter.emit("job-update", update);
        })
        .await;
        *app.state::<AppState>().busy.lock().unwrap() = Busy::Idle;
    });
    Ok(())
}

#[tauri::command]
fn cancel_download(state: State<'_, AppState>) {
    if let Busy::Job(cancel) = &*state.busy.lock().unwrap() {
        cancel.notify_one();
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Checks yt-dlp and spotDL and updates them if they are old. Does nothing if a
/// download is running. Progress goes to the interface as "tools-status" events.
async fn run_update(app: AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut busy = state.busy.lock().unwrap();
        if !matches!(*busy, Busy::Idle) {
            return;
        }
        *busy = Busy::Updating;
    }
    let status = match make_updater(&app) {
        Ok(updater) => {
            let _ = app.emit(
                "tools-status",
                ToolsStatus::from_report(
                    "checking",
                    "Checking for updates…",
                    updater.cached_versions(),
                ),
            );
            let emitter = app.clone();
            let cached = updater.cached_versions();
            let report = updater
                .run(unix_now(), move |msg| {
                    let _ = emitter.emit(
                        "tools-status",
                        ToolsStatus::from_report("checking", msg, UpdateReport::default()),
                    );
                })
                .await;
            ToolsStatus::from_report(
                "idle",
                "",
                UpdateReport {
                    ytdlp_version: report.ytdlp_version.or(cached.ytdlp_version),
                    spotdl_version: report.spotdl_version.or(cached.spotdl_version),
                    ..report
                },
            )
        }
        Err(e) => ToolsStatus::from_report(
            "idle",
            "",
            UpdateReport {
                errors: vec![e],
                ..UpdateReport::default()
            },
        ),
    };
    *app.state::<AppState>().busy.lock().unwrap() = Busy::Idle;
    let _ = app.emit("tools-status", status);
}

/// Last known versions, without starting any program.
#[tauri::command]
fn get_tools_status(app: AppHandle) -> ToolsStatus {
    let report = make_updater(&app)
        .map(|u| u.cached_versions())
        .unwrap_or_default();
    ToolsStatus::from_report("idle", "", report)
}

/// "Check for updates" button.
#[tauri::command]
async fn check_updates(app: AppHandle) {
    run_update(app).await;
}

/// Shows the file in File Explorer or Finder (or opens the folder if the path is a folder).
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let win = path.replace('/', "\\");
        let mut cmd = std::process::Command::new("explorer.exe");
        if p.is_dir() {
            cmd.arg(&win);
        } else {
            cmd.raw_arg(format!("/select,\"{win}\""));
        }
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        // `open -R` reveals the file in Finder; for a folder, just open it
        let mut cmd = std::process::Command::new("open");
        if !p.is_dir() {
            cmd.arg("-R");
        }
        cmd.arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let dir = if p.is_dir() {
            p
        } else {
            p.parent().unwrap_or(p)
        };
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Minimize (the window has no system title bar).
#[tauri::command]
fn minimize(window: tauri::WebviewWindow) {
    let _ = window.minimize();
}

/// Closes the app. On exit all child processes are terminated (Job Object on
/// Windows, process groups on macOS and Linux).
#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn main() {
    // Windows: when the app exits, the OS terminates all child processes.
    // (macOS and Linux: see the Exit handler below.)
    if let Err(e) = ripper_core::jobobject::kill_children_on_exit() {
        eprintln!("Job Object not active: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            // once a day, a few seconds after the window is up (and after each new
            // version of the app), so the first screen is never slowed down
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                let due = make_updater(&handle)
                    .map(|u| {
                        u.forget_copies_of_other_app_versions();
                        u.is_due(unix_now())
                    })
                    .unwrap_or(false);
                if due {
                    run_update(handle).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_out_dir,
            pick_out_dir,
            get_settings,
            set_option,
            import_cookies,
            get_tools_status,
            check_updates,
            start_download,
            cancel_download,
            reveal,
            minimize,
            quit
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Audio Ripper")
        .run(|_app, event| {
            // macOS and Linux: kill the process groups of any running job
            if let tauri::RunEvent::Exit = event {
                ripper_core::children::kill_all();
            }
        });
}
