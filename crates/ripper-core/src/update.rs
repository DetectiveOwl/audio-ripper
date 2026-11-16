//! Keeps yt-dlp and spotDL up to date without a new release of the app.
//!
//! YouTube changes often and an old yt-dlp stops working within weeks. The
//! copies bundled in the installer are only the starting point: updated copies
//! live in a data folder (`data_bin`) that the app can always write to, and
//! `resolve` prefers them over the bundled ones.
//!
//! - yt-dlp updates itself (`yt-dlp -U`, which verifies its own checksum).
//! - spotDL has no self-update: its newest GitHub release asset is downloaded,
//!   checked against the SHA-256 digest GitHub publishes, tried with `--version`
//!   and only then moved into place.
//!
//! Downloads use the system `curl` (Windows 10+ and macOS ship it), which keeps
//! the app free of an HTTP/TLS stack. A new version of the app throws the
//! updated copies away, so its fresher bundled tools win again.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::runner::base_command;

const SPOTDL_API: &str = "https://api.github.com/repos/spotDL/spotify-downloader/releases/latest";
/// How often the automatic check runs.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone)]
struct State {
    /// Version of the app that created the updated copies.
    app_version: String,
    /// Release tag of the spotDL copy in `data_bin`, when we downloaded one.
    spotdl_tag: Option<String>,
    /// Unix time of the last completed check.
    last_check: u64,
    /// Versions seen at the last check, so the UI can show them without
    /// starting any program.
    ytdlp_version: Option<String>,
    spotdl_version: Option<String>,
}

/// What `run` found out.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateReport {
    pub ytdlp_version: Option<String>,
    pub spotdl_version: Option<String>,
    /// Human readable lines such as "yt-dlp 2026.10.26 → 2026.11.10".
    pub updated: Vec<String>,
    pub errors: Vec<String>,
}

pub struct Updater {
    /// Folder of the executable: where the bundled tools are.
    bundled_dir: PathBuf,
    /// Writable folder for the updated copies.
    data_bin: PathBuf,
    app_version: String,
    curl: PathBuf,
    api_url: String,
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

impl Updater {
    pub fn new(bundled_dir: PathBuf, data_bin: PathBuf, app_version: &str) -> Self {
        Self {
            bundled_dir,
            data_bin,
            app_version: app_version.to_string(),
            curl: PathBuf::from("curl"),
            api_url: SPOTDL_API.to_string(),
        }
    }

    #[cfg(test)]
    fn with_curl(mut self, curl: PathBuf, api_url: &str) -> Self {
        self.curl = curl;
        self.api_url = api_url.to_string();
        self
    }

    fn state_path(&self) -> PathBuf {
        self.data_bin.join("state.json")
    }

    fn load_state(&self) -> State {
        std::fs::read_to_string(self.state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, state: &State) -> io::Result<()> {
        std::fs::create_dir_all(&self.data_bin)?;
        let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
        std::fs::write(self.state_path(), json)
    }

    /// Path to use for `name` ("yt-dlp" or "spotdl"): the updated copy if there
    /// is one, otherwise the bundled one.
    pub fn resolve(&self, name: &str) -> PathBuf {
        let updated = self.data_bin.join(exe_name(name));
        if updated.is_file() { updated } else { self.bundled_dir.join(exe_name(name)) }
    }

    /// Call once at startup: if this is a different version of the app than the
    /// one that made the updated copies, drop them (the bundled ones are newer).
    pub fn forget_copies_of_other_app_versions(&self) {
        let state = self.load_state();
        if state.app_version == self.app_version {
            return;
        }
        for name in ["yt-dlp", "spotdl"] {
            let _ = std::fs::remove_file(self.data_bin.join(exe_name(name)));
        }
        if self.data_bin.exists() {
            let _ = self.save_state(&State { app_version: self.app_version.clone(), ..State::default() });
        }
    }

    /// True when the last check is older than `CHECK_EVERY`.
    pub fn is_due(&self, now_secs: u64) -> bool {
        now_secs.saturating_sub(self.load_state().last_check) >= CHECK_EVERY.as_secs()
    }

    /// Versions seen at the last check (None before the first one). Starts no program.
    pub fn cached_versions(&self) -> UpdateReport {
        let s = self.load_state();
        UpdateReport { ytdlp_version: s.ytdlp_version, spotdl_version: s.spotdl_version, ..Default::default() }
    }

    async fn ytdlp_version(&self) -> Option<String> {
        let (code, out) = capture(base_command(&self.resolve("yt-dlp")).arg("--version"), 30).await.ok()?;
        (code == Some(0)).then(|| out.lines().next().unwrap_or("").trim().to_string()).filter(|v| !v.is_empty())
    }

    async fn spotdl_version(&self) -> Option<String> {
        if let Some(tag) = self.load_state().spotdl_tag {
            return Some(tag);
        }
        let (code, out) = capture(base_command(&self.resolve("spotdl")).arg("--version"), 60).await.ok()?;
        if code != Some(0) {
            return None;
        }
        out.split_whitespace().find_map(version_number)
    }

    /// Checks both tools and updates what is old. Never fails as a whole: the
    /// report lists what worked and what did not.
    pub async fn run(&self, now_secs: u64, mut progress: impl FnMut(&str)) -> UpdateReport {
        let mut report = UpdateReport::default();
        let mut state = self.load_state();
        state.app_version = self.app_version.clone();

        progress("Checking yt-dlp…");
        match self.update_ytdlp().await {
            Ok((before, after)) => {
                if let (Some(b), Some(a)) = (&before, &after) {
                    if b != a {
                        report.updated.push(format!("yt-dlp {b} → {a}"));
                    }
                }
                report.ytdlp_version = after.or(before);
            }
            Err(e) => {
                report.errors.push(format!("yt-dlp: {e}"));
                report.ytdlp_version = self.ytdlp_version().await;
            }
        }

        progress("Checking spotDL…");
        match self.update_spotdl(&mut state, &mut progress).await {
            Ok(Some(line)) => report.updated.push(line),
            Ok(None) => {}
            Err(e) => report.errors.push(format!("spotDL: {e}")),
        }
        report.spotdl_version = self.spotdl_version().await;

        state.ytdlp_version = report.ytdlp_version.clone().or(state.ytdlp_version);
        state.spotdl_version = report.spotdl_version.clone().or(state.spotdl_version);
        // only a check that reached the network counts, otherwise retry next launch
        if report.errors.is_empty() {
            state.last_check = now_secs;
        }
        let _ = self.save_state(&state);
        report
    }

    /// Returns (version before, version after).
    async fn update_ytdlp(&self) -> Result<(Option<String>, Option<String>), String> {
        let copy = self.data_bin.join(exe_name("yt-dlp"));
        if !copy.is_file() {
            let bundled = self.bundled_dir.join(exe_name("yt-dlp"));
            std::fs::create_dir_all(&self.data_bin).map_err(|e| e.to_string())?;
            std::fs::copy(&bundled, &copy).map_err(|e| format!("cannot copy {}: {e}", bundled.display()))?;
        }
        let before = self.ytdlp_version().await;
        let (code, out) = capture(base_command(&copy).arg("-U"), 180).await.map_err(|e| e.to_string())?;
        if code != Some(0) {
            let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("update failed");
            return Err(last.trim().chars().take(160).collect());
        }
        Ok((before, self.ytdlp_version().await))
    }

    async fn update_spotdl(
        &self,
        state: &mut State,
        progress: &mut impl FnMut(&str),
    ) -> Result<Option<String>, String> {
        let (_, json) = capture(
            Command::new(&self.curl)
                .args(["-fsSL", "--retry", "2", "-H", "User-Agent: audio-ripper", "-H", "Accept: application/vnd.github+json"])
                .arg(&self.api_url),
            45,
        )
        .await
        .map_err(|e| format!("curl not available: {e}"))?;
        let release = parse_release(&json).ok_or("could not read the latest release (offline?)")?;
        let latest = release.tag.trim_start_matches('v').to_string();

        let current = match &state.spotdl_tag {
            Some(t) => Some(t.clone()),
            None => self.spotdl_version().await,
        };
        if current.as_deref() == Some(latest.as_str()) {
            state.spotdl_tag = Some(latest);
            return Ok(None);
        }

        let asset = pick_spotdl_asset(&release.assets, std::env::consts::OS, cfg!(target_arch = "aarch64"))
            .ok_or("no spotDL build for this system in the latest release")?;
        progress(&format!("Downloading spotDL {latest}…"));
        std::fs::create_dir_all(&self.data_bin).map_err(|e| e.to_string())?;
        let part = self.data_bin.join(format!("{}.part", exe_name("spotdl")));
        let (code, out) = capture(
            Command::new(&self.curl)
                .args(["-fsSL", "--retry", "2", "-H", "User-Agent: audio-ripper", "-o"])
                .arg(&part)
                .arg(&asset.url),
            900,
        )
        .await
        .map_err(|e| e.to_string())?;
        if code != Some(0) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("download failed: {}", out.trim().chars().take(120).collect::<String>()));
        }
        if let Some(expected) = asset.sha256.as_deref() {
            let got = sha256_file(&part).map_err(|e| e.to_string())?;
            if !got.eq_ignore_ascii_case(expected) {
                let _ = std::fs::remove_file(&part);
                return Err("the downloaded file does not match its checksum, discarded".into());
            }
        }
        make_executable(&part).map_err(|e| e.to_string())?;
        // it must at least start, otherwise keep the old one
        let (code, _) = capture(base_command(&part).arg("--version"), 90).await.map_err(|e| e.to_string())?;
        if code != Some(0) {
            let _ = std::fs::remove_file(&part);
            return Err("the new spotDL does not start, kept the old one".into());
        }
        let target = self.data_bin.join(exe_name("spotdl"));
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        let from = current.unwrap_or_else(|| "bundled".into());
        state.spotdl_tag = Some(latest.clone());
        Ok(Some(format!("spotDL {from} → {latest}")))
    }
}

/// Runs a command to the end (or `secs`), returning exit code and all its output.
async fn capture(cmd: &mut Command, secs: u64) -> io::Result<(Option<i32>, String)> {
    cmd.kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(secs), cmd.output())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out"))??;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}

fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// "4.2.11" (also "v4.2.11" or "4.2.11,") if the word looks like a version number.
fn version_number(word: &str) -> Option<String> {
    let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric()).trim_start_matches('v');
    let ok = w.contains('.') && w.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    ok.then(|| w.to_string())
}

#[derive(Debug, PartialEq)]
struct Release {
    tag: String,
    assets: Vec<Asset>,
}

#[derive(Debug, PartialEq, Clone)]
struct Asset {
    name: String,
    url: String,
    /// Lowercase hex SHA-256, when GitHub publishes it for the asset.
    sha256: Option<String>,
}

fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    let assets = v
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some(Asset {
                name: a.get("name")?.as_str()?.to_string(),
                url: a.get("browser_download_url")?.as_str()?.to_string(),
                sha256: a
                    .get("digest")
                    .and_then(|d| d.as_str())
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .map(str::to_lowercase),
            })
        })
        .collect();
    Some(Release { tag, assets })
}

/// Picks the spotDL build for this system (same rules as the fetch-binaries scripts).
fn pick_spotdl_asset(assets: &[Asset], os: &str, arm: bool) -> Option<Asset> {
    let has = |a: &Asset, s: &str| a.name.to_lowercase().contains(s);
    match os {
        "windows" => assets.iter().find(|a| has(a, "win") && has(a, ".exe")).cloned(),
        "macos" => {
            let mac: Vec<&Asset> = assets.iter().filter(|a| has(a, "darwin") || has(a, "macos")).collect();
            let by_arch = mac.iter().find(|a| has(a, "arm64") == arm).copied();
            by_arch.or_else(|| mac.first().copied()).cloned()
        }
        "linux" => assets.iter().find(|a| has(a, "linux")).cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), url: format!("https://x/{name}"), sha256: None }
    }

    #[test]
    fn version_words() {
        assert_eq!(version_number("4.2.11"), Some("4.2.11".into()));
        assert_eq!(version_number("v4.2.11,"), Some("4.2.11".into()));
        assert_eq!(version_number("spotDL"), None);
        assert_eq!(version_number("4"), None);
        assert_eq!(version_number("4..1"), None);
    }

    #[test]
    fn release_json() {
        let json = r#"{"tag_name":"v4.2.11","assets":[
            {"name":"spotdl-4.2.11-win32.exe","browser_download_url":"https://x/win.exe","digest":"sha256:ABCDEF"},
            {"name":"spotdl-4.2.11-linux","browser_download_url":"https://x/linux"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.tag, "v4.2.11");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.assets[0].sha256.as_deref(), Some("abcdef"));
        assert_eq!(r.assets[1].sha256, None);
        assert!(parse_release("not json").is_none());
        assert!(parse_release(r#"{"message":"rate limit exceeded"}"#).is_none());
    }

    #[test]
    fn asset_choice() {
        let all = [
            asset("spotdl-4.2.11-win32.exe"),
            asset("spotdl-4.2.11-linux"),
            asset("spotdl-4.2.11-darwin"),
            asset("spotdl-4.2.11-darwin-arm64"),
        ];
        assert_eq!(pick_spotdl_asset(&all, "windows", false).unwrap().name, "spotdl-4.2.11-win32.exe");
        assert_eq!(pick_spotdl_asset(&all, "macos", true).unwrap().name, "spotdl-4.2.11-darwin-arm64");
        assert_eq!(pick_spotdl_asset(&all, "macos", false).unwrap().name, "spotdl-4.2.11-darwin");
        assert_eq!(pick_spotdl_asset(&all, "linux", false).unwrap().name, "spotdl-4.2.11-linux");
        assert!(pick_spotdl_asset(&all[..2], "macos", false).is_none());
        assert!(pick_spotdl_asset(&all, "freebsd", false).is_none());
    }

    #[test]
    fn checksum() {
        let d = std::env::temp_dir().join(format!("ripper-sha-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("f");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ripper-upd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolve_prefers_the_updated_copy() {
        let d = tmp("resolve");
        let u = Updater::new(d.join("app"), d.join("data"), "1.0.0");
        assert_eq!(u.resolve("yt-dlp"), d.join("app").join(exe_name("yt-dlp")));
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::write(d.join("data").join(exe_name("yt-dlp")), b"x").unwrap();
        assert_eq!(u.resolve("yt-dlp"), d.join("data").join(exe_name("yt-dlp")));
    }

    #[test]
    fn a_new_app_version_drops_the_old_copies() {
        let d = tmp("newapp");
        let data = d.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join(exe_name("yt-dlp")), b"x").unwrap();
        let old = Updater::new(d.join("app"), data.clone(), "1.0.0");
        old.save_state(&State { app_version: "1.0.0".into(), ..State::default() }).unwrap();
        old.forget_copies_of_other_app_versions();
        assert!(data.join(exe_name("yt-dlp")).exists(), "same version: kept");

        let new = Updater::new(d.join("app"), data.clone(), "1.1.0");
        new.forget_copies_of_other_app_versions();
        assert!(!data.join(exe_name("yt-dlp")).exists(), "other version: dropped");
        assert_eq!(new.load_state().app_version, "1.1.0");
    }

    #[test]
    fn due_after_a_day() {
        let d = tmp("due");
        let u = Updater::new(d.join("app"), d.join("data"), "1.0.0");
        assert!(u.is_due(1_000_000), "never checked");
        u.save_state(&State { last_check: 1_000_000, ..State::default() }).unwrap();
        assert!(!u.is_due(1_000_000 + 3600));
        assert!(u.is_due(1_000_000 + 24 * 3600));
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(path: &Path, body: &str) {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        const FAKE_YTDLP: &str = r#"d="$(dirname "$0")"
v="$(cat "$d/ver" 2>/dev/null || echo 2026.01.01)"
case "$1" in
  --version) echo "$v" ;;
  -U) echo 2026.02.02 > "$d/ver"; echo "Updated yt-dlp to stable@2026.02.02" ;;
esac"#;

        #[tokio::test]
        async fn ytdlp_is_copied_then_updated_in_the_data_folder() {
            let d = tmp("ytdlp");
            std::fs::create_dir_all(d.join("app")).unwrap();
            script(&d.join("app").join("yt-dlp"), FAKE_YTDLP);
            // no spotDL anywhere and no network: only yt-dlp is exercised
            let u = Updater::new(d.join("app"), d.join("data"), "1.0.0")
                .with_curl(d.join("no-such-curl"), "http://unused");
            let report = u.run(5_000_000, |_| {}).await;
            assert_eq!(report.ytdlp_version.as_deref(), Some("2026.02.02"));
            assert_eq!(report.updated, vec!["yt-dlp 2026.01.01 → 2026.02.02".to_string()]);
            // the bundled copy is untouched, the updated one is used from now on
            assert_eq!(std::fs::read_to_string(d.join("app").join("ver")).ok(), None);
            assert_eq!(u.resolve("yt-dlp"), d.join("data").join("yt-dlp"));
            // curl was missing, so the check is not recorded and runs again next time
            assert!(!report.errors.is_empty());
            assert!(u.is_due(5_000_001));
        }

        #[tokio::test]
        async fn spotdl_is_downloaded_verified_and_installed() {
            let d = tmp("spotdl");
            std::fs::create_dir_all(d.join("app")).unwrap();
            script(&d.join("app").join("yt-dlp"), FAKE_YTDLP);
            // the "new spotDL" the fake curl will hand out
            let payload = d.join("payload");
            script(&payload, r#"echo "spotDL 4.9.9""#);
            let sum = sha256_file(&payload).unwrap();
            let os = match std::env::consts::OS { "macos" => "darwin", o => o };
            let json = format!(
                r#"{{"tag_name":"v4.9.9","assets":[{{"name":"spotdl-4.9.9-{os}","browser_download_url":"file://{p}","digest":"sha256:{sum}"}}]}}"#,
                p = payload.display()
            );
            std::fs::write(d.join("api.json"), json).unwrap();
            // fake curl: `-o FILE URL` copies the payload, otherwise prints the API json
            let curl = d.join("curl");
            script(
                &curl,
                &format!(
                    r#"out=""
while [ $# -gt 1 ]; do [ "$1" = "-o" ] && out="$2"; shift; done
if [ -n "$out" ]; then cp "{p}" "$out"; else cat "{a}"; fi"#,
                    p = payload.display(),
                    a = d.join("api.json").display()
                ),
            );
            let u = Updater::new(d.join("app"), d.join("data"), "1.0.0").with_curl(curl, "http://unused");
            let report = u.run(7_000_000, |_| {}).await;
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(report.spotdl_version.as_deref(), Some("4.9.9"));
            assert_eq!(u.resolve("spotdl"), d.join("data").join("spotdl"));
            assert!(!d.join("data").join("spotdl.part").exists());
            assert!(!u.is_due(7_000_100), "a clean check is remembered");
            assert_eq!(u.cached_versions().spotdl_version.as_deref(), Some("4.9.9"));
            assert_eq!(u.cached_versions().ytdlp_version.as_deref(), Some("2026.02.02"));

            // a second run finds it up to date and downloads nothing
            let again = u.run(9_000_000, |_| {}).await;
            assert!(again.updated.is_empty(), "{:?}", again.updated);
        }

        #[tokio::test]
        async fn a_wrong_checksum_is_rejected_and_the_old_spotdl_kept() {
            let d = tmp("badsum");
            std::fs::create_dir_all(d.join("app")).unwrap();
            script(&d.join("app").join("yt-dlp"), FAKE_YTDLP);
            script(&d.join("app").join("spotdl"), r#"echo "spotDL 4.0.0""#);
            let payload = d.join("payload");
            script(&payload, r#"echo "evil""#);
            let os = match std::env::consts::OS { "macos" => "darwin", o => o };
            let json = format!(
                r#"{{"tag_name":"v4.9.9","assets":[{{"name":"spotdl-{os}","browser_download_url":"x","digest":"sha256:{z}"}}]}}"#,
                z = "0".repeat(64)
            );
            std::fs::write(d.join("api.json"), json).unwrap();
            let curl = d.join("curl");
            script(
                &curl,
                &format!(
                    r#"out=""
while [ $# -gt 1 ]; do [ "$1" = "-o" ] && out="$2"; shift; done
if [ -n "$out" ]; then cp "{p}" "$out"; else cat "{a}"; fi"#,
                    p = payload.display(),
                    a = d.join("api.json").display()
                ),
            );
            let u = Updater::new(d.join("app"), d.join("data"), "1.0.0").with_curl(curl, "http://unused");
            let report = u.run(8_000_000, |_| {}).await;
            assert!(report.errors.iter().any(|e| e.contains("checksum")), "{:?}", report.errors);
            assert_eq!(u.resolve("spotdl"), d.join("app").join("spotdl"), "the bundled spotDL stays in use");
            assert!(!d.join("data").join("spotdl.part").exists());
        }
    }
}
