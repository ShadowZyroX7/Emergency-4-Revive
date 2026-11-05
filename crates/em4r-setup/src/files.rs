//! File operations in the game folder. No UI, no elevation, no registry.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};

pub const BACKUP_DIR: &str = "_em4revive_backup";
pub const UNINSTALLER: &str = "EM4Revive-Uninstall.exe";
const MARKER: &str = "installed.txt";
const FOREIGN_DLL: &str = "dinput8.dll.fremd";
const NEW_SUFFIX: &str = ".em4r_new";
const OLD_SUFFIX: &str = ".em4r_old";

pub struct Payload<'a> {
    pub dll: &'a [u8],
    pub launcher: &'a [u8],
    pub default_config: &'a [u8],
    pub version: &'a str,
}

pub fn is_game_dir(dir: &Path) -> bool {
    dir.join("Em4.exe").is_file()
}

pub fn uninstaller_path(dir: &Path) -> PathBuf {
    dir.join(BACKUP_DIR).join(UNINSTALLER)
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02X}")).collect()
}

/// Installed version: `None` = not installed, `Some("")` = installed by a
/// version that did not record its number yet.
pub fn installed_version(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(BACKUP_DIR).join(MARKER)).ok()?;
    let version = text.lines().find_map(|l| l.trim_start_matches('\u{feff}').strip_prefix("version="));
    Some(version.unwrap_or("").trim().to_owned())
}

fn marker_hash(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(BACKUP_DIR).join(MARKER)).ok()?;
    text.lines().find_map(|l| l.strip_prefix("sha256=")).map(|s| s.trim().to_owned())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Replace several files as one unit: everything is staged first, and any
/// failure while swapping restores the previous files.
fn replace_all(files: &[(PathBuf, &[u8])]) -> io::Result<()> {
    for (path, data) in files {
        fs::write(with_suffix(path, NEW_SUFFIX), data)?;
    }
    let mut swapped: Vec<&Path> = Vec::new();
    let mut result = Ok(());
    for (path, _) in files {
        let old = with_suffix(path, OLD_SUFFIX);
        let _ = fs::remove_file(&old);
        let had_old = path.exists();
        let step = (|| {
            if had_old {
                fs::rename(path, &old)?;
            }
            if let Err(e) = fs::rename(with_suffix(path, NEW_SUFFIX), path) {
                if had_old {
                    let _ = fs::rename(&old, path);
                }
                return Err(e);
            }
            Ok(())
        })();
        match step {
            Ok(()) => swapped.push(path),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if result.is_err() {
        for path in swapped {
            let old = with_suffix(path, OLD_SUFFIX);
            let _ = fs::remove_file(path);
            if old.exists() {
                let _ = fs::rename(&old, path);
            }
        }
    }
    for (path, _) in files {
        let _ = fs::remove_file(with_suffix(path, NEW_SUFFIX));
        // Fails harmlessly if the old file is still mapped; next run retries.
        if result.is_ok() {
            let _ = fs::remove_file(with_suffix(path, OLD_SUFFIX));
        }
    }
    result
}

/// Install or update. Never touches an existing `em4revive.toml`.
pub fn install(dir: &Path, payload: &Payload<'_>, setup_exe: Option<&Path>) -> io::Result<()> {
    if payload.dll.is_empty() || payload.launcher.is_empty() {
        return Err(Error::new(ErrorKind::InvalidData, "Dieses Setup enthält keine Mod-Dateien (Entwickler-Build)."));
    }
    if !is_game_dir(dir) {
        return Err(Error::new(ErrorKind::NotFound, format!("Em4.exe nicht gefunden in {}", dir.display())));
    }
    let backup = dir.join(BACKUP_DIR);
    fs::create_dir_all(&backup)?;

    for name in ["em4.cfg", "ReShade.ini"] {
        let (src, dst) = (dir.join(name), backup.join(name));
        if src.is_file() && !dst.exists() {
            fs::copy(src, dst)?;
        }
    }

    // A dinput8.dll without our marker belongs to another mod: keep it.
    let dll_path = dir.join("dinput8.dll");
    let foreign = backup.join(FOREIGN_DLL);
    if dll_path.is_file() && installed_version(dir).is_none() && !foreign.exists() {
        fs::copy(&dll_path, &foreign)?;
    }

    replace_all(&[(dll_path, payload.dll), (dir.join("EM4Revive.exe"), payload.launcher)])?;

    let config = dir.join("em4revive.toml");
    if !config.exists() {
        fs::write(config, payload.default_config)?;
    }

    if let Some(exe) = setup_exe {
        let target = uninstaller_path(dir);
        let same = fs::canonicalize(exe).ok().zip(fs::canonicalize(&target).ok()).is_some_and(|(a, b)| a == b);
        if !same {
            fs::copy(exe, target)?;
        }
    }

    fs::write(
        backup.join(MARKER),
        format!("version={}\nsha256={}\n", payload.version, sha256_hex(payload.dll)),
    )
}

/// Remove the mod and restore a foreign dinput8.dll. Same rules as
/// scripts/uninstall.ps1: `em4revive.toml`, crash dumps and backups stay.
pub fn uninstall(dir: &Path) -> io::Result<()> {
    let backup = dir.join(BACKUP_DIR);
    let dll = dir.join("dinput8.dll");
    if dll.is_file() {
        let actual = sha256_hex(&fs::read(&dll)?);
        if marker_hash(dir).is_some_and(|h| !h.eq_ignore_ascii_case(&actual)) {
            fs::copy(&dll, backup.join(format!("dinput8.dll.{actual}")))?;
        }
        fs::remove_file(&dll)?;
    }
    let foreign = backup.join(FOREIGN_DLL);
    if foreign.is_file() {
        fs::rename(foreign, &dll)?;
    }
    for name in ["dinput8.pdb", "EM4Revive.exe", "em4revive_launcher.toml", "em4revive.log", "em4revive_stats.csv"] {
        match fs::remove_file(dir.join(name)) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    // Unfinished submod downloads can be large.
    let _ = fs::remove_dir_all(dir.join("_em4revive_download"));
    let _ = fs::remove_file(backup.join(MARKER));
    Ok(())
}

/// Can we create files here without elevation?
pub fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".em4r_schreibtest");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_game(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("em4r_setup_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Em4.exe"), b"game").unwrap();
        dir
    }

    fn payload(v: &'static str) -> Payload<'static> {
        Payload { dll: v.as_bytes(), launcher: b"launcher", default_config: b"default", version: v }
    }

    #[test]
    fn update_keeps_user_config_and_records_version() {
        let dir = temp_game("update");
        assert_eq!(installed_version(&dir), None);
        install(&dir, &payload("1.0.0"), None).unwrap();
        assert_eq!(fs::read(dir.join("em4revive.toml")).unwrap(), b"default");
        fs::write(dir.join("em4revive.toml"), b"mine").unwrap();
        install(&dir, &payload("1.1.0"), None).unwrap();
        assert_eq!(fs::read(dir.join("em4revive.toml")).unwrap(), b"mine");
        assert_eq!(fs::read(dir.join("dinput8.dll")).unwrap(), b"1.1.0");
        assert_eq!(installed_version(&dir).as_deref(), Some("1.1.0"));
        assert!(!dir.join("dinput8.dll.em4r_old").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn foreign_dll_is_kept_and_restored() {
        let dir = temp_game("foreign");
        fs::write(dir.join("dinput8.dll"), b"other mod").unwrap();
        install(&dir, &payload("1.0.0"), None).unwrap();
        install(&dir, &payload("1.1.0"), None).unwrap();
        assert_eq!(fs::read(dir.join(BACKUP_DIR).join(FOREIGN_DLL)).unwrap(), b"other mod");
        uninstall(&dir).unwrap();
        assert_eq!(fs::read(dir.join("dinput8.dll")).unwrap(), b"other mod");
        assert!(!dir.join("EM4Revive.exe").exists());
        assert_eq!(installed_version(&dir), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_swap_restores_previous_files() {
        let dir = temp_game("rollback");
        install(&dir, &payload("1.0.0"), None).unwrap();
        // A non-empty directory where the second file's backup goes makes the swap fail.
        let blocker = dir.join("blocker.bin");
        fs::create_dir_all(with_suffix(&blocker, OLD_SUFFIX).join("voll")).unwrap();
        fs::write(&blocker, b"x").unwrap();
        let files = [(dir.join("dinput8.dll"), &b"neu"[..]), (blocker.clone(), &b"neu"[..])];
        assert!(replace_all(&files).is_err());
        assert_eq!(fs::read(dir.join("dinput8.dll")).unwrap(), b"1.0.0");
        assert_eq!(fs::read(&blocker).unwrap(), b"x");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_wrong_folder_and_empty_payload() {
        let dir = temp_game("wrong");
        let empty = Payload { dll: &[], launcher: &[], default_config: &[], version: "0" };
        assert!(install(&dir, &empty, None).is_err());
        fs::remove_file(dir.join("Em4.exe")).unwrap();
        assert!(install(&dir, &payload("1.0.0"), None).is_err());
        assert!(!dir.join("dinput8.dll").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_marker_of_script_install() {
        let dir = temp_game("marker");
        fs::create_dir_all(dir.join(BACKUP_DIR)).unwrap();
        fs::write(dir.join(BACKUP_DIR).join(MARKER), "\u{feff}installiert=2026\nsha256=AB\n").unwrap();
        assert_eq!(installed_version(&dir).as_deref(), Some(""));
        fs::remove_dir_all(&dir).unwrap();
    }
}
