//! GitHub Releases access shared by the self-update and the submods page.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = concat!("EM4Revive-Launcher/", env!("CARGO_PKG_VERSION"));

/// Error text of a download or install the user stopped on purpose.
pub const CANCELLED: &str = "Abgebrochen";

#[derive(Deserialize)]
pub struct ApiRelease {
    pub tag_name: String,
    pub assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
pub struct ApiAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

impl ApiAsset {
    /// Lowercase hex SHA-256, when GitHub provides one.
    pub fn sha256(&self) -> Option<String> {
        self.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase)
    }

    /// The asset really belongs to `repo`.
    pub fn is_from(&self, repo: &str) -> bool {
        self.browser_download_url.starts_with(&format!("https://github.com/{repo}/releases/download/"))
    }
}

pub fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .https_only(true)
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build())
}

/// The API answer for the newest release; `None` if nothing is published yet.
pub fn latest_release_json(repo: &str) -> Result<Option<String>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    match agent()?.get(&url).set("Accept", "application/vnd.github+json").call() {
        Ok(response) => response.into_string().map(Some).map_err(|e| e.to_string()),
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download to `dest`. The file only appears there once size and hash match.
/// An interrupted download stays as `.part` and continues on the next call.
pub fn download(
    url: &str,
    size: u64,
    sha256: Option<&str>,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    let part = dest.with_extension("part");
    let mut done = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if done > size {
        std::fs::remove_file(&part).map_err(|e| e.to_string())?;
        done = 0;
    }
    if done < size {
        let mut request = agent()?.get(url);
        if done > 0 {
            request = request.set("Range", &format!("bytes={done}-"));
        }
        let response = request.call().map_err(|e| e.to_string())?;
        if response.status() != 206 {
            done = 0;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(done == 0)
            .open(&part)
            .map_err(|e| e.to_string())?;
        file.seek(std::io::SeekFrom::End(0)).map_err(|e| e.to_string())?;
        let mut reader = response.into_reader();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.to_owned());
            }
            let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(done);
        }
    }

    // From here on the partial file is either good or useless.
    let verified = if done != size {
        // A shorter file can continue later; a longer one was removed above.
        return Err(format!("Download unvollständig ({done} von {size} Bytes)."));
    } else if let Some(expected) = sha256 {
        hash_file(&part).and_then(|actual| {
            if actual == expected {
                Ok(())
            } else {
                Err("Download beschädigt (Prüfsumme stimmt nicht).".to_owned())
            }
        })
    } else {
        Ok(())
    };
    let result = verified.and_then(|()| std::fs::rename(&part, dest).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real network: TLS, the GitHub API, the redirect to the CDN, resuming
    /// and hash checking, against a public project. `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_download_from_public_release() {
        let body = latest_release_json("BurntSushi/ripgrep").unwrap().unwrap();
        let api: ApiRelease = serde_json::from_str(&body).unwrap();
        let asset = api.assets.iter().find(|a| a.size > 1000).unwrap();
        let digest = asset.sha256();
        let dest = std::env::temp_dir().join(format!("em4r_live_{}.bin", std::process::id()));
        let go = AtomicBool::new(false);

        // Interrupted download: only the start is on disk, then it continues.
        let start = agent()
            .unwrap()
            .get(&asset.browser_download_url)
            .set("Range", "bytes=0-999")
            .call()
            .unwrap();
        let mut bytes = Vec::new();
        start.into_reader().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len(), 1000);
        std::fs::write(dest.with_extension("part"), &bytes).unwrap();
        let mut first_progress = None;
        download(&asset.browser_download_url, asset.size, digest.as_deref(), &dest, &go, |d| {
            first_progress.get_or_insert(d);
        })
        .unwrap();
        assert!(first_progress.unwrap() > 1000, "muss fortsetzen, nicht neu beginnen");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), asset.size);
        std::fs::remove_file(&dest).unwrap();

        // A wrong hash must not leave a file behind.
        assert!(download(&asset.browser_download_url, asset.size, Some("00"), &dest, &go, |_| {}).is_err());
        assert!(!dest.exists());

        // Cancelling keeps nothing at the destination.
        let stop = AtomicBool::new(true);
        assert_eq!(
            download(&asset.browser_download_url, asset.size, None, &dest, &stop, |_| {}).unwrap_err(),
            CANCELLED
        );
        assert!(!dest.exists());
        let _ = std::fs::remove_file(dest.with_extension("part"));
    }
}
