// Su Windows, in release, niente finestra di console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ripper_core::runner::{run_job, JobSpec, Tools};
use ripper_core::{validate_url, Format};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Notify;

/// Stato condiviso: al massimo un job alla volta.
#[derive(Default)]
struct AppState {
    active: Mutex<Option<Arc<Notify>>>,
}

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    out_dir: Option<String>,
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

/// Cartella scelta dall'utente, altrimenti Musica, altrimenti Download.
fn resolve_out_dir(app: &AppHandle) -> PathBuf {
    if let Some(dir) = load_settings(app).out_dir {
        return PathBuf::from(dir);
    }
    app.path()
        .audio_dir()
        .or_else(|_| app.path().download_dir())
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Gli strumenti esterni stanno accanto all'eseguibile (sidecar di Tauri).
fn resolve_tools(app: &AppHandle) -> Result<Tools, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("Cartella dell'app non trovata.")?;
    let name = |n: &str| dir.join(if cfg!(windows) { format!("{n}.exe") } else { n.to_string() });
    let deno = name("deno");
    let cookies = config_dir(app).ok().map(|d| d.join("cookies.txt")).filter(|p| p.exists());
    Ok(Tools {
        ytdlp: name("yt-dlp"),
        spotdl: name("spotdl"),
        ffmpeg: name("ffmpeg"),
        deno: deno.exists().then_some(deno),
        cookies,
    })
}

#[tauri::command]
fn get_out_dir(app: AppHandle) -> String {
    resolve_out_dir(&app).to_string_lossy().into_owned()
}

#[tauri::command]
async fn pick_out_dir(app: AppHandle) -> Result<Option<String>, String> {
    let app2 = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else { return Ok(None) };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let dir = path.to_string_lossy().into_owned();
    save_settings(&app, &Settings { out_dir: Some(dir.clone()) })?;
    Ok(Some(dir))
}

#[tauri::command]
async fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    fmt: String,
) -> Result<(), String> {
    let format = Format::parse(&fmt).ok_or("Formato non valido: usa MP3 o WAV.")?;
    let (url, source) = validate_url(&url)?;
    let out_dir = resolve_out_dir(&app);
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Cartella di destinazione non utilizzabile: {e}"))?;
    let tools = resolve_tools(&app)?;

    let cancel = Arc::new(Notify::new());
    {
        let mut active = state.active.lock().unwrap();
        if active.is_some() {
            return Err("Un'estrazione è già in corso.".into());
        }
        *active = Some(cancel.clone());
    }

    let spec = JobSpec { url, source, format, out_dir };
    tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        run_job(spec, &tools, cancel, move |update| {
            let _ = emitter.emit("job-update", update);
        })
        .await;
        app.state::<AppState>().active.lock().unwrap().take();
    });
    Ok(())
}

#[tauri::command]
fn cancel_download(state: State<'_, AppState>) {
    if let Some(cancel) = state.active.lock().unwrap().as_ref() {
        cancel.notify_one();
    }
}

/// Mostra il file in Esplora risorse (o apre la cartella se il percorso è una cartella).
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
    #[cfg(not(windows))]
    {
        let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

fn main() {
    // Alla chiusura dell'app Windows termina tutti i processi figli.
    if let Err(e) = ripper_core::jobobject::kill_children_on_exit() {
        eprintln!("Job Object non attivo: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_out_dir,
            pick_out_dir,
            start_download,
            cancel_download,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("errore durante l'avvio di Audio Ripper");
}
