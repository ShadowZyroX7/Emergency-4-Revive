//! Faster `logfile.txt`.
//!
//! Em4.exe's log function (0x73B4xx) does, for every single line:
//! `_wfopen(path, L"a")` → `fprintf` → `fclose`. Opening and closing the file
//! costs ~0.2 ms each time; during loading that happens ~12 000 times
//! (> 2 s), and AUBMP's scripts keep logging several lines per second while
//! playing.
//!
//! We redirect exactly those two calls: the file is opened once and kept
//! open; instead of closing it we `fflush` it, so every line is still on
//! disk immediately (nothing is lost on a crash).

use em4r_core::patch::{self, PatchResult};
use em4r_core::{info, warn};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const WFOPEN_CALL_SITE: usize = 0x73B52B; // call [0x9D21EC] = MSVCR71!_wfopen
const WFOPEN_IAT: usize = 0x9D21EC;
const FCLOSE_CALL_SITE: usize = 0x73B54C; // call [0x9D2478] = MSVCR71!fclose
const FCLOSE_IAT: usize = 0x9D2478;

type WfopenFn = unsafe extern "C" fn(*const u16, *const u16) -> usize;
type FileFn = unsafe extern "C" fn(usize) -> i32;

static REAL_WFOPEN: AtomicUsize = AtomicUsize::new(0);
static REAL_FCLOSE: AtomicUsize = AtomicUsize::new(0);
static REAL_FFLUSH: AtomicUsize = AtomicUsize::new(0);

struct Cached {
    file: usize,
    path: Vec<u16>,
    mode: Vec<u16>,
}

static CACHED: Mutex<Option<Cached>> = Mutex::new(None);

pub fn init() {
    let resolve = |name: &str| em4r_core::util::loaded_proc("msvcr71.dll", name).unwrap_or(0);
    let (wfopen, fclose, fflush) = (resolve("_wfopen"), resolve("fclose"), resolve("fflush"));
    if wfopen == 0 || fclose == 0 || fflush == 0 {
        warn!("Schnelles Log: MSVCR71-Funktionen nicht gefunden – übersprungen");
        return;
    }
    REAL_WFOPEN.store(wfopen, Ordering::Release);
    REAL_FCLOSE.store(fclose, Ordering::Release);
    REAL_FFLUSH.store(fflush, Ordering::Release);

    // Both patches or none (an open file without the matching close patch
    // would leak a handle per line).
    // SAFETY: DllMain at process start; the game code has not run yet.
    unsafe {
        let ok_open = patch::matches(WFOPEN_CALL_SITE, &indirect(WFOPEN_IAT));
        let ok_close = patch::matches(FCLOSE_CALL_SITE, &indirect(FCLOSE_IAT));
        if !(ok_open && ok_close) {
            warn!("Schnelles Log: unbekannte Em4.exe-Version – übersprungen");
            return;
        }
        let a = patch::redirect_indirect_call(WFOPEN_CALL_SITE, WFOPEN_IAT, log_wfopen as *const () as usize);
        let b = patch::redirect_indirect_call(FCLOSE_CALL_SITE, FCLOSE_IAT, log_fclose as *const () as usize);
        if a == PatchResult::Applied && b == PatchResult::Applied {
            info!("Patch aktiv: Schnelles Log (logfile.txt bleibt geöffnet)");
        } else {
            warn!("Schnelles Log: Patch unvollständig ({a:?} / {b:?})");
        }
    }
}

fn indirect(slot: usize) -> [u8; 6] {
    let s = (slot as u32).to_le_bytes();
    [0xFF, 0x15, s[0], s[1], s[2], s[3]]
}

unsafe fn wstr(p: *const u16) -> Vec<u16> {
    let mut v = Vec::new();
    if p.is_null() {
        return v;
    }
    let mut i = 0;
    while *p.add(i) != 0 && i < 1024 {
        v.push(*p.add(i));
        i += 1;
    }
    v
}

unsafe extern "C" fn log_wfopen(path: *const u16, mode: *const u16) -> usize {
    let (p, m) = (wstr(path), wstr(mode));
    if let Ok(mut g) = CACHED.lock() {
        if let Some(c) = g.as_ref() {
            if c.path == p && c.mode == m {
                return c.file;
            }
        }
        let real: WfopenFn = core::mem::transmute(REAL_WFOPEN.load(Ordering::Acquire));
        let f = real(path, mode);
        if f != 0 && g.is_none() {
            *g = Some(Cached { file: f, path: p, mode: m });
        }
        return f;
    }
    let real: WfopenFn = core::mem::transmute(REAL_WFOPEN.load(Ordering::Acquire));
    real(path, mode)
}

unsafe extern "C" fn log_fclose(file: usize) -> i32 {
    let is_cached = CACHED.lock().map(|g| g.as_ref().is_some_and(|c| c.file == file)).unwrap_or(false);
    if is_cached {
        let flush: FileFn = core::mem::transmute(REAL_FFLUSH.load(Ordering::Acquire));
        flush(file);
        return 0;
    }
    let close: FileFn = core::mem::transmute(REAL_FCLOSE.load(Ordering::Acquire));
    close(file)
}
