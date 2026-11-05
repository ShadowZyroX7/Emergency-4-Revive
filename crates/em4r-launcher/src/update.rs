//! Self-update from GitHub Releases.
//!
//! The newest published release is read from the GitHub API. If it is newer
//! than this build, its `EM4Revive-Setup.exe` is downloaded, verified and run
//! with `--update`; the setup replaces the files and restarts the launcher.
//!
//! The repository is fixed at build time (`EM4R_GITHUB_REPO`, "owner/name").
//! Without it, the launcher never goes online.

use crate::github;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub const REPO: Option<&str> = option_env!("EM4R_GITHUB_REPO");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET_NAME: &str = "EM4Revive-Setup.exe";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex SHA-256 from GitHub, when it provides one.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Built without a repository, or not running from the game folder.
    Disabled,
    Checking,
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64 },
    Ready { release: Release, path: PathBuf },
    Failed(String),
}

/// "v1.2.3" → (1, 2, 3). Anything else is not a version we act on.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn release_from_api(json: &str, repo: &str, current: &str) -> Result<Option<Release>, String> {
    let api: github::ApiRelease = serde_json::from_str(json).map_err(|e| format!("Unerwartete Antwort von GitHub: {e}"))?;
    let latest = parse_version(&api.tag_name).ok_or_else(|| format!("Ungültige Versionsnummer: {}", api.tag_name))?;
    let installed = parse_version(current).ok_or("Ungültige eigene Versionsnummer")?;
    if latest <= installed {
        return Ok(None);
    }
    let asset = api
        .assets
        .into_iter()
        .find(|a| a.name == ASSET_NAME)
        .ok_or_else(|| format!("Release {} enthält keine {ASSET_NAME}", api.tag_name))?;
    if !asset.is_from(repo) {
        return Err("Download-Adresse gehört nicht zu diesem Projekt.".into());
    }
    let sha256 = asset.sha256();
    Ok(Some(Release {
        version: format!("{}.{}.{}", latest.0, latest.1, latest.2),
        url: asset.browser_download_url,
        size: asset.size,
        sha256,
    }))
}

fn latest_release(repo: &str) -> Result<Option<Release>, String> {
    match github::latest_release_json(repo)? {
        Some(body) => release_from_api(&body, repo, CURRENT_VERSION),
        None => Ok(None),
    }
}

/// Remove setups of earlier updates from the temp folder.
fn remove_old_downloads() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("EM4Revive-Setup-") && name.ends_with(".exe") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Background worker state, shared with the UI thread.
#[derive(Clone)]
pub struct Updater {
    state: Arc<Mutex<State>>,
    repaint: eframe::egui::Context,
}

impl Updater {
    pub fn new(repaint: eframe::egui::Context, initial: State) -> Self {
        Self { state: Arc::new(Mutex::new(initial)), repaint }
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
        self.repaint.request_repaint();
    }

    pub fn start_check(&self) {
        let Some(repo) = REPO else { return };
        self.set(State::Checking);
        let this = self.clone();
        std::thread::spawn(move || {
            remove_old_downloads();
            this.set(match latest_release(repo) {
                Ok(Some(release)) => State::Available(release),
                Ok(None) => State::UpToDate,
                Err(e) => State::Failed(format!("Update-Prüfung fehlgeschlagen: {e}")),
            });
        });
    }

    pub fn start_download(&self, release: Release) {
        self.set(State::Downloading { release: release.clone(), done: 0 });
        let this = self.clone();
        std::thread::spawn(move || {
            let path = std::env::temp_dir().join(format!("EM4Revive-Setup-{}.exe", release.version));
            let result = github::download(&release.url, release.size, release.sha256.as_deref(), &path, &AtomicBool::new(false), |done| {
                this.set(State::Downloading { release: release.clone(), done });
            });
            this.set(match result {
                Ok(()) => State::Ready { release, path },
                Err(e) => State::Failed(format!("Update-Download fehlgeschlagen: {e}")),
            });
        });
    }
}

/// Start the downloaded setup. It waits for this process to exit.
pub fn launch_setup(setup: &Path, game_dir: &Path) -> std::io::Result<()> {
    std::process::Command::new(setup)
        .arg("--update")
        .arg("--dir")
        .arg(game_dir)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_T: &str = "me/em4-revive";
    const GOOD_URL: &str = "https://github.com/me/em4-revive/releases/download/v0.3.0/EM4Revive-Setup.exe";

    fn json(tag: &str, url: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","body":"x","assets":[
              {{"name":"dinput8.pdb","browser_download_url":"https://github.com/me/em4-revive/releases/download/{tag}/dinput8.pdb","size":1,"digest":null}},
              {{"name":"EM4Revive-Setup.exe","browser_download_url":"{url}","size":42,"digest":"sha256:ABCDEF"}}]}}"#
        )
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("neu"), None);
    }

    #[test]
    fn newer_release_is_offered_with_setup_asset() {
        let r = release_from_api(&json("v0.3.0", GOOD_URL), REPO_T, "0.2.0").unwrap().unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.url, GOOD_URL);
        assert_eq!(r.size, 42);
        assert_eq!(r.sha256.as_deref(), Some("abcdef"));
    }

    #[test]
    fn same_or_older_release_is_ignored() {
        assert_eq!(release_from_api(&json("v0.2.0", GOOD_URL), REPO_T, "0.2.0").unwrap(), None);
        assert_eq!(release_from_api(&json("v0.1.9", GOOD_URL), REPO_T, "0.2.0").unwrap(), None);
    }

    #[test]
    fn foreign_download_url_is_rejected() {
        assert!(release_from_api(&json("v0.3.0", "https://evil.example/x.exe"), REPO_T, "0.2.0").is_err());
    }

    #[test]
    fn release_without_setup_is_an_error() {
        let body = r#"{"tag_name":"v0.3.0","assets":[]}"#;
        assert!(release_from_api(body, REPO_T, "0.2.0").is_err());
    }
}
