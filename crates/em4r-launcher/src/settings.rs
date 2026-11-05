//! Launcher settings (`em4revive_launcher.toml` next to the launcher).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folder of the last played mod; empty = original EM4.
    pub last_mod: String,
    /// Game folder, if the launcher does not live inside it.
    pub game_dir: Option<PathBuf>,
    /// Last chosen start mode.
    pub last_mode: Option<em4r_core::launch::StartMode>,
    /// Ask before installing updates instead of doing it automatically.
    pub manual_updates: bool,
    /// Version the launcher last tried to update to. If it is still offered
    /// after a restart, the attempt failed: ask instead of looping.
    pub last_update_attempt: String,
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(t) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, t);
        }
    }
}

/// Find the EM4 folder: next to the launcher, the saved path, or the
/// default Steam location.
pub fn find_game_dir(exe_dir: Option<&Path>, saved: Option<&Path>) -> Option<PathBuf> {
    let default = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\EMERGENCY 4 Deluxe");
    let user_lib = PathBuf::from(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe");
    [exe_dir.map(Path::to_path_buf), saved.map(Path::to_path_buf), Some(user_lib), Some(default)]
        .into_iter()
        .flatten()
        .find(|d| d.join("Em4.exe").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("em4r_launcher_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.toml");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            last_mod: "AUBMP".into(),
            game_dir: None,
            last_mode: Some(em4r_core::launch::StartMode::FreeplayEndless),
            ..Default::default()
        };
        s.save(&p);
        assert_eq!(Settings::load(&p), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn game_dir_requires_em4_exe() {
        assert!(find_game_dir(Some(Path::new(r"C:\Windows")), None).map_or(true, |d| d.join("Em4.exe").is_file()));
    }
}
