//! Self-update from GitHub Releases.
//!
//! The newest published release is read from the GitHub API. If it is newer
//! than this build, its `EM4Revive-Setup.exe` is downloaded, verified and run
//! with `--update`; the setup replaces the files and restarts the launcher.
//!
//! The repository is fixed at build time (`EM4R_GITHUB_REPO`, "owner/name").
//! Without it, the launcher never goes online.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const REPO: Option<&str> = option_env!("EM4R_GITHUB_REPO");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET_NAME: &str = "EM4Revive-Setup.exe";
const USER_AGENT: &str = concat!("EM4Revive-Launcher/", env!("CARGO_PKG_VERSION"));

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

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

/// "v1.2.3" → (1, 2, 3). Anything else is not a version we act on.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn release_from_api(json: &str, repo: &str, current: &str) -> Result<Option<Release>, String> {
    let api: ApiRelease = serde_json::from_str(json).map_err(|e| format!("Unerwartete Antwort von GitHub: {e}"))?;
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
    let expected_prefix = format!("https://github.com/{repo}/releases/download/");
    if !asset.browser_download_url.starts_with(&expected_prefix) {
        return Err("Download-Adresse gehört nicht zu diesem Projekt.".into());
    }
    Ok(Some(Release {
        version: format!("{}.{}.{}", latest.0, latest.1, latest.2),
        url: asset.browser_download_url,
        size: asset.size,
        sha256: asset.digest.and_then(|d| d.strip_prefix("sha256:").map(str::to_ascii_lowercase)),
    }))
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .https_only(true)
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build())
}

fn latest_release(repo: &str) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    match agent()?.get(&url).set("Accept", "application/vnd.github+json").call() {
        Ok(response) => {
            let body = response.into_string().map_err(|e| e.to_string())?;
            release_from_api(&body, repo, CURRENT_VERSION)
        }
        // No published release yet.
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download to `dest`. The file only appears there once size and hash match.
fn download(url: &str, size: u64, sha256: Option<&str>, dest: &Path, mut progress: impl FnMut(u64)) -> Result<(), String> {
    let part = dest.with_extension("part");
    let response = agent()?.get(url).call().map_err(|e| e.to_string())?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let copied: Result<(), String> = loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e.to_string()),
        };
        if let Err(e) = file.write_all(&buffer[..n]) {
            break Err(e.to_string());
        }
        hasher.update(&buffer[..n]);
        done += n as u64;
        progress(done);
    };
    drop(file);
    let actual = hex(&hasher.finalize());
    let verified = copied.and_then(|()| {
        if done != size {
            Err(format!("Download unvollständig ({done} von {size} Bytes)."))
        } else if sha256.is_some_and(|expected| expected != actual) {
            Err("Download beschädigt (Prüfsumme stimmt nicht).".to_owned())
        } else {
            std::fs::rename(&part, dest).map_err(|e| e.to_string())
        }
    });
    if verified.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    verified
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
            let result = download(&release.url, release.size, release.sha256.as_deref(), &path, |done| {
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

    /// Real network: TLS, the GitHub API, the redirect to the CDN and hash
    /// checking, against a public project. `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_download_from_public_release() {
        let body = agent()
            .unwrap()
            .get("https://api.github.com/repos/BurntSushi/ripgrep/releases/latest")
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        let api: ApiRelease = serde_json::from_str(&body).unwrap();
        let asset = api.assets.iter().min_by_key(|a| a.size).unwrap();
        let digest = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:"));
        let dest = std::env::temp_dir().join(format!("em4r_live_{}.bin", std::process::id()));
        download(&asset.browser_download_url, asset.size, digest, &dest, |_| {}).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), asset.size);
        std::fs::remove_file(&dest).unwrap();
        // A wrong hash must not leave a file behind.
        assert!(download(&asset.browser_download_url, asset.size, Some("00"), &dest, |_| {}).is_err());
        assert!(!dest.exists());
    }
}
