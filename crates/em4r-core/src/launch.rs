//! One-shot start request from the launcher to the mod
//! (`em4revive_launch.toml` in the game folder).
//!
//! The launcher writes it right before starting Em4.exe; the mod reads and
//! deletes it at startup, so a later normal start (e.g. via Steam) is never
//! affected. Requests older than [`MAX_AGE_SECS`] are ignored.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "em4revive_launch.toml";
pub const MAX_AGE_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    /// Just the main menu (no automatic start).
    Menu,
    /// Freeplay, endless ("Endlosspiel").
    FreeplayEndless,
    /// Freeplay, challenge.
    FreeplayChallenge,
}

impl StartMode {
    /// Game type argument of Em4.exe's StartGame (from its loading texts:
    /// 0 = ID_LOADING_FREEPLAY_NEVERENDING, 1 = ID_LOADING_FREEPLAY_CHALLENGE).
    pub fn game_type(self) -> Option<i32> {
        match self {
            StartMode::Menu => None,
            StartMode::FreeplayEndless => Some(0),
            StartMode::FreeplayChallenge => Some(1),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StartMode::Menu => "Hauptmenü",
            StartMode::FreeplayEndless => "Freies Spiel – Endlos",
            StartMode::FreeplayChallenge => "Freies Spiel – Challenge",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub mode: StartMode,
    /// Unix time when the launcher wrote the request.
    pub created: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl LaunchRequest {
    pub fn new(mode: StartMode) -> Self {
        Self { mode, created: now() }
    }

    pub fn write(&self, game_dir: &Path) -> std::io::Result<()> {
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(game_dir.join(FILE_NAME), text)
    }

    /// Read and delete the request. `Ok(None)` if there is none or it is stale.
    pub fn take(game_dir: &Path) -> Result<Option<Self>, String> {
        let path = game_dir.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let _ = std::fs::remove_file(&path);
        let req: LaunchRequest = toml::from_str(&text).map_err(|e| e.to_string())?;
        let age = now().saturating_sub(req.created);
        Ok((age <= MAX_AGE_SECS).then_some(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("em4r_launch_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_take_is_one_shot() {
        let d = tmp("oneshot");
        LaunchRequest::new(StartMode::FreeplayEndless).write(&d).unwrap();
        let r = LaunchRequest::take(&d).unwrap().unwrap();
        assert_eq!(r.mode, StartMode::FreeplayEndless);
        assert_eq!(LaunchRequest::take(&d).unwrap(), None); // deleted
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn stale_requests_are_ignored() {
        let d = tmp("stale");
        LaunchRequest { mode: StartMode::FreeplayChallenge, created: now() - MAX_AGE_SECS - 5 }.write(&d).unwrap();
        assert_eq!(LaunchRequest::take(&d).unwrap(), None);
        assert!(!d.join(FILE_NAME).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn game_types_match_em4() {
        assert_eq!(StartMode::FreeplayEndless.game_type(), Some(0));
        assert_eq!(StartMode::FreeplayChallenge.game_type(), Some(1));
        assert_eq!(StartMode::Menu.game_type(), None);
        let text = toml::to_string(&LaunchRequest { mode: StartMode::FreeplayEndless, created: 1 }).unwrap();
        assert!(text.contains("freeplay_endless"), "{text}");
    }
}
