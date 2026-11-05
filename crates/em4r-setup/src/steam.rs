//! Finds the EM4 folder through the Steam registry key and library list.

use crate::files::is_game_dir;
use std::path::{Path, PathBuf};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::RegKey;

const GAME_FOLDER: &str = r"steamapps\common\EMERGENCY 4 Deluxe";

/// All `"path"` values of a libraryfolders.vdf.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').skip(1);
            let key = parts.next()?;
            let value = parts.nth(1)?;
            key.eq_ignore_ascii_case("path").then(|| PathBuf::from(value.replace(r"\\", r"\")))
        })
        .collect()
}

fn steam_roots() -> Vec<PathBuf> {
    let read = |hive, key: &str, value: &str| -> Option<PathBuf> {
        let v: String = RegKey::predef(hive).open_subkey(key).ok()?.get_value(value).ok()?;
        Some(PathBuf::from(v))
    };
    [
        read(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        read(HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam", "InstallPath"),
        Some(PathBuf::from(r"C:\Program Files (x86)\Steam")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn library_candidates(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    if let Ok(vdf) = std::fs::read_to_string(root.join(r"steamapps\libraryfolders.vdf")) {
        libs.extend(library_paths(&vdf));
    }
    libs
}

/// The game folder, if the setup can find it without asking.
pub fn find_game() -> Option<PathBuf> {
    steam_roots().iter().flat_map(|r| library_candidates(r)).map(|l| l.join(GAME_FOLDER)).find(|d| is_game_dir(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\Spiele\\SteamLibrary"
	}
}"#;
        assert_eq!(
            library_paths(vdf),
            vec![PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\Spiele\SteamLibrary")]
        );
    }
}
