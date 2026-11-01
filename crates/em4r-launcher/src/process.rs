//! The running game: either a child we spawned directly, or an Em4.exe that
//! Steam started for us (needed for the Steam overlay / F12 screenshots).

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Steam app id of EMERGENCY 4 Deluxe (appmanifest_757210.acf).
pub const STEAM_APP_ID: u32 = 757210;
/// How long to wait for Steam to start Em4.exe.
const STEAM_START_TIMEOUT: Duration = Duration::from_secs(90);

type Handle = *mut c_void;

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    pid: u32,
    heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_pid: u32,
    pri_class_base: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snap: Handle, e: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snap: Handle, e: *mut ProcessEntry32W) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(p: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        len: *mut u32,
    ) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x2;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const INVALID_HANDLE: Handle = -1isize as Handle;

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// steam.exe from the registry (HKCU\Software\Valve\Steam\SteamExe).
pub fn steam_exe() -> Option<PathBuf> {
    let mut buf = [0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: valid buffers; RegGetValueW null-terminates REG_SZ data.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(r"Software\Valve\Steam").as_ptr(),
            wide("SteamExe").as_ptr(),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if r != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]).replace('/', "\\"));
    p.is_file().then_some(p)
}

/// Arguments for steam.exe that start EM4 with the given game arguments.
pub fn steam_arguments(game_args: &[String]) -> Vec<String> {
    let mut v = vec!["-applaunch".to_string(), STEAM_APP_ID.to_string()];
    v.extend(game_args.iter().cloned());
    v
}

/// Ids of running processes whose image is `<game_dir>\Em4.exe`.
fn em4_processes(game_dir: &Path) -> Vec<u32> {
    let want = game_dir.join("Em4.exe").to_string_lossy().to_lowercase();
    let mut out = Vec::new();
    // SAFETY: standard Toolhelp enumeration with a correctly sized entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE {
            return out;
        }
        let mut e: ProcessEntry32W = core::mem::zeroed();
        e.size = core::mem::size_of::<ProcessEntry32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let n = e.exe_file.iter().position(|&c| c == 0).unwrap_or(0);
            if String::from_utf16_lossy(&e.exe_file[..n]).eq_ignore_ascii_case("Em4.exe")
                && image_path(e.pid).is_some_and(|p| p.to_lowercase() == want)
            {
                out.push(e.pid);
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub enum GameProcess {
    Direct(Child),
    Steam {
        requested: Instant,
        /// Em4.exe processes that already ran before we asked Steam.
        before: Vec<u32>,
        handle: Option<usize>,
    },
}

impl GameProcess {
    pub fn spawn_direct(game_dir: &Path, args: &[String]) -> std::io::Result<Self> {
        Command::new(game_dir.join("Em4.exe")).args(args).current_dir(game_dir).spawn().map(GameProcess::Direct)
    }

    pub fn spawn_via_steam(steam: &Path, game_dir: &Path, args: &[String]) -> std::io::Result<Self> {
        let before = em4_processes(game_dir);
        Command::new(steam).args(steam_arguments(args)).spawn()?;
        Ok(GameProcess::Steam { requested: Instant::now(), before, handle: None })
    }

    pub fn via_steam(&self) -> bool {
        matches!(self, GameProcess::Steam { .. })
    }

    /// Still waiting for Steam to start the game.
    pub fn waiting_for_steam(&self) -> bool {
        matches!(self, GameProcess::Steam { handle: None, .. })
    }

    /// Has the game ended (or did Steam never start it)?
    pub fn finished(&mut self, game_dir: &Path) -> bool {
        match self {
            GameProcess::Direct(c) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
            GameProcess::Steam { requested, before, handle } => {
                if handle.is_none() {
                    let new = em4_processes(game_dir).into_iter().find(|p| !before.contains(p));
                    if let Some(pid) = new {
                        // SAFETY: plain OpenProcess; closed in Drop.
                        let h = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
                        if !h.is_null() {
                            *handle = Some(h as usize);
                        }
                    }
                    return handle.is_none() && requested.elapsed() > STEAM_START_TIMEOUT;
                }
                let h = handle.unwrap() as Handle;
                // SAFETY: valid process handle with SYNCHRONIZE access.
                unsafe { WaitForSingleObject(h, 0) == 0 }
            }
        }
    }
}

impl Drop for GameProcess {
    fn drop(&mut self) {
        if let GameProcess::Steam { handle: Some(h), .. } = self {
            // SAFETY: handle opened by us.
            unsafe { CloseHandle(*h as Handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_args() {
        let a = steam_arguments(&["-game".into(), "-mod".into(), "AUBMP".into()]);
        assert_eq!(a, ["-applaunch", "757210", "-game", "-mod", "AUBMP"]);
    }

    #[test]
    fn steam_lookup_does_not_panic() {
        if let Some(p) = steam_exe() {
            assert!(p.to_string_lossy().to_lowercase().ends_with("steam.exe"));
        }
    }

    #[test]
    fn no_em4_in_random_dir() {
        assert!(em4_processes(Path::new(r"C:\does\not\exist")).is_empty());
    }
}
