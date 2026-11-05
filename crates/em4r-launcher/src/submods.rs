//! Submods from GitHub Releases: find the newest version, download, unpack and
//! put it into `Mods\<folder>`.
//!
//! The running game or a closed launcher must never leave a half-written mod
//! behind, so everything happens in `<game>\_em4revive_download` on the same
//! drive and the finished folder is moved into place in one step. Savegames
//! always carry over. A folder this launcher did not install (for example a
//! working copy) is never deleted, only moved to the backup folder.

use crate::github::{self, CANCELLED};
use serde::Deserialize;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const DOWNLOAD_DIR: &str = "_em4revive_download";
const BACKUP_DIR: &str = r"_em4revive_backup\Submods";
const MARKER: &str = "em4revive_submod.toml";
/// Room for archive + unpacked files; ROBMP unpacks to about 3x its size.
const SPACE_FACTOR: u64 = 5;

pub struct SubmodDef {
    pub name: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
    /// Folder below `Mods`.
    pub folder: &'static str,
}

pub const SUBMODS: &[SubmodDef] = &[SubmodDef {
    name: "ROBMP Submod",
    description: "Eine Submodifikation der Mod Rosenburg für Emergency 4.",
    repo: "ShadowZyroX7/ROBMP-Submod",
    folder: "ROBMP Submod",
}];

#[derive(Debug, Clone, PartialEq)]
pub enum Installed {
    No,
    /// The folder exists, but this launcher did not put it there.
    Foreign,
    Version(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Latest {
    pub tag: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    Download { done: u64, total: u64 },
    Unpack { done: u64, total: u64 },
    Install,
}

#[derive(Deserialize)]
struct Marker {
    version: String,
}

pub fn installed(game_dir: &Path, def: &SubmodDef) -> Installed {
    let dir = game_dir.join("Mods").join(def.folder);
    if !dir.is_dir() {
        return Installed::No;
    }
    std::fs::read_to_string(dir.join(MARKER))
        .ok()
        .and_then(|t| toml::from_str::<Marker>(&t).ok())
        .map_or(Installed::Foreign, |m| Installed::Version(m.version))
}

fn latest_from_api(json: &str, repo: &str) -> Result<Latest, String> {
    let api: github::ApiRelease =
        serde_json::from_str(json).map_err(|e| format!("Unerwartete Antwort von GitHub: {e}"))?;
    let asset = api
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".7z"))
        .ok_or_else(|| format!("Release {} enthält kein .7z-Archiv.", api.tag_name))?;
    if !asset.is_from(repo) {
        return Err("Download-Adresse gehört nicht zu diesem Projekt.".into());
    }
    Ok(Latest {
        tag: api.tag_name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256: asset.sha256(),
    })
}

/// The newest published version; `None` if there is no release yet.
pub fn latest(def: &SubmodDef) -> Result<Option<Latest>, String> {
    github::latest_release_json(def.repo)?.map(|json| latest_from_api(&json, def.repo)).transpose()
}

/// Where an archive entry goes. Rejects anything that could leave `root`.
fn entry_path(root: &Path, name: &str) -> Option<PathBuf> {
    let relative = PathBuf::from(name.replace('/', "\\"));
    let safe = relative.components().all(|c| matches!(c, Component::Normal(_)));
    (safe && relative.components().next().is_some()).then(|| root.join(relative))
}

fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and `free` outlives the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, core::ptr::null_mut(), core::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

fn other(message: impl Into<std::borrow::Cow<'static, str>>) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Other(message.into())
}

/// Unpack a `.7z` into `dest`.
fn extract(archive: &Path, dest: &Path, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    use sevenz_rust2::{ArchiveReader, Error, Password};
    let describe = |e: Error| e.to_string();
    let mut reader = ArchiveReader::open(archive, Password::empty()).map_err(describe)?;
    // Each worker needs its own dictionary. In this 32-bit process several of
    // them run out of address space (seen with ROBMP), so unpack with one.
    reader.set_thread_count(1);
    let total: u64 = reader.archive().files.iter().map(|f| f.size()).sum();
    let mut done = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    reader
        .for_each_entries(|entry, data: &mut dyn Read| {
            if cancel.load(Ordering::Relaxed) {
                return Err(other(CANCELLED));
            }
            let path = entry_path(dest, entry.name())
                .ok_or_else(|| other(format!("Unzulässiger Pfad im Archiv: {}", entry.name())))?;
            if entry.is_directory() {
                std::fs::create_dir_all(&path).map_err(|e| other(e.to_string()))?;
                return Ok(true);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| other(e.to_string()))?;
            }
            let file = std::fs::File::create(&path).map_err(|e| other(e.to_string()))?;
            let mut out = io::BufWriter::new(file);
            loop {
                let n = data.read(&mut buffer).map_err(|e| other(e.to_string()))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buffer[..n]).map_err(|e| other(e.to_string()))?;
                done += n as u64;
                progress(done, total);
                if cancel.load(Ordering::Relaxed) {
                    return Err(other(CANCELLED));
                }
            }
            out.flush().map_err(|e| other(e.to_string()))?;
            Ok(true)
        })
        .map_err(|e| {
            let text = e.to_string();
            if text.contains(CANCELLED) {
                CANCELLED.to_owned()
            } else {
                text
            }
        })
}

/// The mod root inside the unpacked files: right there, or one folder down.
fn find_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join("e4mod.info").is_file() {
        return Ok(staging.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::fs::read_dir(staging)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    match dirs.as_slice() {
        [only] if only.join("e4mod.info").is_file() => Ok(only.clone()),
        _ => Err("Das Archiv enthält keine Mod (keine e4mod.info gefunden).".to_owned()),
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Move `new_root` to `target`, carrying over the savegames of the old folder.
/// The old folder goes to `parking` and is deleted there unless `keep_old`.
/// Any failure leaves the old folder in place.
fn swap_in(new_root: &Path, target: &Path, parking: &Path, keep_old: bool) -> io::Result<()> {
    if !target.exists() {
        return std::fs::rename(new_root, target);
    }
    if let Some(parent) = parking.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(target, parking)?;
    if let Err(e) = std::fs::rename(new_root, target) {
        let _ = std::fs::rename(parking, target);
        return Err(e);
    }
    let saves = parking.join("Save");
    if saves.is_dir() {
        if let Err(e) = copy_tree(&saves, &target.join("Save")) {
            // Hand the new files back so a retry does not have to unpack again.
            let _ = std::fs::rename(target, new_root);
            let _ = std::fs::rename(parking, target);
            return Err(e);
        }
    }
    if !keep_old {
        let _ = std::fs::remove_dir_all(parking);
    }
    Ok(())
}

fn unix_time() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Download and install `latest`. Returns where a foreign folder was moved to.
pub fn install(
    game_dir: &Path,
    def: &SubmodDef,
    latest: &Latest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Option<PathBuf>, String> {
    let work = game_dir.join(DOWNLOAD_DIR);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let tag: String =
        latest.tag.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    let archive = work.join(format!("{}-{tag}.7z", def.folder));
    let partial = archive.with_extension("part");

    // Leftovers of older versions and interrupted unpacking.
    for entry in std::fs::read_dir(&work).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path != archive && path != partial {
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }

    if !archive.exists() {
        let needed = latest.size.saturating_mul(SPACE_FACTOR);
        if free_space(&work).is_some_and(|free| free < needed) {
            return Err(format!("Nicht genug Speicherplatz: Es werden etwa {} GB gebraucht.", needed.div_ceil(1 << 30)));
        }
        progress(Progress::Download { done: 0, total: latest.size });
        github::download(&latest.url, latest.size, latest.sha256.as_deref(), &archive, cancel, |done| {
            progress(Progress::Download { done, total: latest.size });
        })?;
    }

    let staging = work.join("neu");
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    extract(&archive, &staging, cancel, |done, total| progress(Progress::Unpack { done, total }))?;
    let root = find_root(&staging)?;
    let marker = format!("version = {:?}\n", latest.tag);
    std::fs::write(root.join(MARKER), marker).map_err(|e| e.to_string())?;

    progress(Progress::Install);
    let target = game_dir.join("Mods").join(def.folder);
    let keep_old = target.exists() && !matches!(installed(game_dir, def), Installed::Version(_));
    let parking = if keep_old {
        game_dir.join(BACKUP_DIR).join(format!("{}_{}", def.folder, unix_time()))
    } else {
        work.join("alt")
    };
    swap_in(&root, &target, &parking, keep_old).map_err(|e| {
        format!("Der Mod-Ordner konnte nicht ersetzt werden. Läuft das Spiel noch? ({e})")
    })?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(keep_old.then_some(parking))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const DEF: SubmodDef = SubmodDef { name: "T", description: "", repo: "me/t", folder: "T Mod" };

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("em4r_submod_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn newest_release_uses_the_archive_and_any_tag_style() {
        let json = r#"{"tag_name":"V0.1","assets":[
            {"name":"notes.txt","browser_download_url":"https://github.com/me/t/releases/download/V0.1/notes.txt","size":1,"digest":null},
            {"name":"ROBMP.Submod.7z","browser_download_url":"https://github.com/me/t/releases/download/V0.1/ROBMP.Submod.7z","size":9,"digest":"sha256:ABC"}]}"#;
        let l = latest_from_api(json, "me/t").unwrap();
        assert_eq!(l.tag, "V0.1");
        assert!(l.url.ends_with("ROBMP.Submod.7z"));
        assert_eq!(l.sha256.as_deref(), Some("abc"));
        assert!(latest_from_api(json, "other/repo").is_err());
        assert!(latest_from_api(r#"{"tag_name":"V1","assets":[]}"#, "me/t").is_err());
    }

    #[test]
    fn archive_paths_cannot_escape() {
        let root = Path::new(r"C:\ziel");
        assert_eq!(entry_path(root, "Models/a b.v3o"), Some(PathBuf::from(r"C:\ziel\Models\a b.v3o")));
        for bad in ["../x", r"..\x", "a/../../x", r"C:\Windows\x", r"\x", "/x", ""] {
            assert_eq!(entry_path(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn finds_mod_root_directly_or_one_level_down() {
        let dir = temp("root");
        assert!(find_root(&dir).is_err());
        write(&dir.join("Hülle").join("e4mod.info"), "x");
        assert_eq!(find_root(&dir).unwrap(), dir.join("Hülle"));
        write(&dir.join("e4mod.info"), "x");
        assert_eq!(find_root(&dir).unwrap(), dir);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detects_install_state() {
        let game = temp("state");
        assert_eq!(installed(&game, &DEF), Installed::No);
        write(&game.join(r"Mods\T Mod\e4mod.info"), "x");
        assert_eq!(installed(&game, &DEF), Installed::Foreign);
        write(&game.join(r"Mods\T Mod").join(MARKER), "version = \"V0.1\"\n");
        assert_eq!(installed(&game, &DEF), Installed::Version("V0.1".into()));
        fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn update_replaces_files_and_keeps_savegames() {
        let base = temp("swap");
        let (new, target, parking) = (base.join("neu"), base.join("mod"), base.join("alt"));
        write(&new.join("datei.txt"), "neu");
        write(&new.join(r"Save\campaignprogress.xml"), "aus dem archiv");
        write(&target.join("datei.txt"), "alt");
        write(&target.join("nur_alt.txt"), "alt");
        write(&target.join(r"Save\campaignprogress.xml"), "mein fortschritt");
        write(&target.join(r"Save\spiel 1.sav"), "mein spiel");
        swap_in(&new, &target, &parking, false).unwrap();
        assert_eq!(fs::read_to_string(target.join("datei.txt")).unwrap(), "neu");
        assert!(!target.join("nur_alt.txt").exists());
        assert_eq!(fs::read_to_string(target.join(r"Save\campaignprogress.xml")).unwrap(), "mein fortschritt");
        assert_eq!(fs::read_to_string(target.join(r"Save\spiel 1.sav")).unwrap(), "mein spiel");
        assert!(!parking.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn foreign_folder_is_moved_away_untouched() {
        let base = temp("foreign");
        let (new, target, parking) = (base.join("neu"), base.join("mod"), base.join(r"sicherung\mod_1"));
        write(&new.join("datei.txt"), "neu");
        write(&target.join("arbeit.txt"), "mein arbeitsstand");
        write(&target.join(r"Save\spiel.sav"), "mein spiel");
        swap_in(&new, &target, &parking, true).unwrap();
        assert_eq!(fs::read_to_string(parking.join("arbeit.txt")).unwrap(), "mein arbeitsstand");
        assert_eq!(fs::read_to_string(parking.join(r"Save\spiel.sav")).unwrap(), "mein spiel");
        assert_eq!(fs::read_to_string(target.join(r"Save\spiel.sav")).unwrap(), "mein spiel");
        assert!(!target.join("arbeit.txt").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn failed_swap_keeps_the_old_folder() {
        let base = temp("fail");
        let (new, target, parking) = (base.join("fehlt"), base.join("mod"), base.join("alt"));
        write(&target.join("datei.txt"), "alt");
        assert!(swap_in(&new, &target, &parking, false).is_err());
        assert_eq!(fs::read_to_string(target.join("datei.txt")).unwrap(), "alt");
        assert!(!parking.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    /// Unpacks a real archive: `EM4R_TEST_ARCHIVE=... EM4R_TEST_OUT=...
    /// cargo test --release -- --ignored real_archive`.
    #[test]
    #[ignore]
    fn real_archive() {
        let archive = PathBuf::from(std::env::var("EM4R_TEST_ARCHIVE").unwrap());
        let out = PathBuf::from(std::env::var("EM4R_TEST_OUT").unwrap());
        let stop = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let mut last = (0, 0);
        extract(&archive, &out, &stop, |d, t| last = (d, t)).unwrap();
        assert_eq!(last.0, last.1);
        println!("{} Bytes in {:?}", last.1, started.elapsed());
        find_root(&out).unwrap();
    }

    /// The whole way from GitHub into a game folder:
    /// `EM4R_TEST_GAME=... cargo test --release -- --ignored live_install`.
    #[test]
    #[ignore]
    fn live_install() {
        let game = PathBuf::from(std::env::var("EM4R_TEST_GAME").unwrap());
        let def = &SUBMODS[0];
        let release = latest(def).unwrap().unwrap();
        let stop = AtomicBool::new(false);
        let mut steps = Vec::new();
        let moved = install(&game, def, &release, &stop, |p| {
            let step = std::mem::discriminant(&p);
            if steps.last() != Some(&step) {
                steps.push(step);
            }
        })
        .unwrap();
        println!("Schritte: {}, verschoben nach: {moved:?}", steps.len());
        assert_eq!(installed(&game, def), Installed::Version(release.tag));
        assert!(!game.join(DOWNLOAD_DIR).exists());
    }
}
