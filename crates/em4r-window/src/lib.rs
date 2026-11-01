//! Borderless fullscreen and fast Alt+Tab for Em4.exe.
//!
//! Verified behaviour of the original game (Em4.exe 1.3 / vBase71.dll):
//! * Em4.exe creates the main window ("BLWNDCLS", `0x73B17F`) with
//!   `WS_EX_TOPMOST`; vBase71 later sets `WS_POPUP|WS_VISIBLE|WS_CLIPSIBLINGS`,
//!   moves it to 0,0,w,h and switches the display mode with
//!   `ChangeDisplaySettingsA` (delay-load import).
//! * While the window is inactive and fullscreen, the main loop (`0x477C08`)
//!   calls `0x5CF390`: it switches to windowed mode, `Sleep(500)`, minimises,
//!   waits another 1.5 s, then pauses until reactivated, switches the mode
//!   back, makes the window `HWND_TOPMOST` and waits 1 s more.
//! * vBase71 calls `SystemParametersInfoA(SPI_SETSCREENSAVERRUNNING, 1)`, which
//!   makes Windows suppress the Print key and Windows-key shortcuts.
//!
//! In borderless mode (game resolution == desktop resolution) the mode switch
//! is skipped, the window is not topmost, never minimised, and `0x5CF390` is
//! replaced by a light pause that only waits for reactivation.

pub mod logic;

use core::ffi::c_void;
use em4r_core::ffi::{BOOL, HWND};
use em4r_core::hook::IatHook;
use em4r_core::{iat_hook, info, warn};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

static ORIG_CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_CDS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIP: AtomicUsize = AtomicUsize::new(0);

static CFG_BORDERLESS: AtomicBool = AtomicBool::new(false);
static CFG_PAUSE: AtomicBool = AtomicBool::new(true);
static CFG_SYSKEYS: AtomicBool = AtomicBool::new(false);

/// Main game window (0 until created).
static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
/// Borderless allowed for the game window (cleared if a different resolution is requested).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The fullscreen mode switch was skipped: the game believes it is fullscreen.
static FAKED: AtomicBool = AtomicBool::new(false);
/// Desktop resolution (width << 16 | height), read when the window is created.
static DESKTOP: AtomicU32 = AtomicU32::new(0);
static LOGGED_CDS: AtomicBool = AtomicBool::new(false);
static LOGGED_MIN: AtomicBool = AtomicBool::new(false);
static LOGGED_SPI: AtomicBool = AtomicBool::new(false);
static PAUSES: AtomicU32 = AtomicU32::new(0);

// ---- Em4.exe addresses (fixed base 0x400000, verified with em4r-re) ----
const BACKGROUND_CALL_SITE: usize = 0x477C08; // call 0x5CF390 (cdecl, 1 arg)
const BACKGROUND_ORIGINAL: usize = 0x5CF390;
const GET_SCREEN: usize = 0x40E750; // BLScreen singleton: +0x18 hwnd, +0x25 active
const GET_INPUT: usize = 0x479AD0; // input manager singleton
const INPUT_SUSPEND: usize = 0x8E5110; // thiscall (as at 0x5CF39E)
const INPUT_RESUME: usize = 0x8E4410; // thiscall (as at 0x5CF5D6)
const KEEP_RUNNING: usize = 0xBBF1C0; // byte; WM_CLOSE handler 0x73B320 clears it

#[repr(C)]
struct Msg {
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "user32")]
extern "system" {
    fn EnumDisplaySettingsA(device: *const u8, mode: u32, dm: *mut u8) -> BOOL;
    fn PeekMessageA(msg: *mut Msg, hwnd: HWND, min: u32, max: u32, remove: u32) -> BOOL;
    fn TranslateMessage(msg: *const Msg) -> BOOL;
    fn DispatchMessageA(msg: *const Msg) -> isize;
    fn IsWindow(hwnd: HWND) -> BOOL;
    fn GetForegroundWindow() -> HWND;
}

type CreateWindowExAFn = unsafe extern "system" fn(
    u32, *const u8, *const u8, u32, i32, i32, i32, i32, HWND, *mut c_void, *mut c_void, *mut c_void,
) -> HWND;
type SetWindowPosFn = unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> BOOL;
type ShowWindowFn = unsafe extern "system" fn(HWND, i32) -> BOOL;
type CdsFn = unsafe extern "system" fn(*mut u8, u32) -> i32;
type SpiFn = unsafe extern "system" fn(u32, u32, *mut c_void, u32) -> BOOL;
type ClipCursorFn = unsafe extern "system" fn(*const Rect) -> BOOL;

pub fn configure(cfg: &em4r_core::config::Window) {
    CFG_BORDERLESS.store(cfg.borderless, Ordering::Relaxed);
    CFG_PAUSE.store(cfg.pause_in_background, Ordering::Relaxed);
    CFG_SYSKEYS.store(cfg.allow_system_keys, Ordering::Relaxed);
}

/// True when any part of this module is switched on.
pub fn wanted(cfg: &em4r_core::config::Window) -> bool {
    cfg.borderless || cfg.allow_system_keys
}

pub fn hooks() -> Vec<IatHook> {
    let mut v = Vec::new();
    if CFG_BORDERLESS.load(Ordering::Relaxed) {
        v.extend([
            iat_hook!("user32.dll", "CreateWindowExA", hk_create_window_ex_a, ORIG_CREATE_WINDOW),
            iat_hook!("user32.dll", "SetWindowPos", hk_set_window_pos, ORIG_SET_WINDOW_POS),
            iat_hook!("user32.dll", "ShowWindow", hk_show_window, ORIG_SHOW_WINDOW),
            iat_hook!("user32.dll", "ChangeDisplaySettingsA", hk_change_display_settings_a, ORIG_CDS),
            iat_hook!("user32.dll", "ClipCursor", hk_clip_cursor, ORIG_CLIP),
        ]);
    }
    if CFG_SYSKEYS.load(Ordering::Relaxed) {
        v.push(iat_hook!("user32.dll", "SystemParametersInfoA", hk_system_parameters_info_a, ORIG_SPI));
    }
    v
}

/// Replace the slow minimise-and-wait routine in Em4.exe's main loop.
/// Call only in Em4.exe, during DllMain (no other thread runs that code).
pub fn patch_em4() {
    if !CFG_BORDERLESS.load(Ordering::Relaxed) {
        return;
    }
    use em4r_core::patch::{redirect_call, PatchResult};
    // SAFETY: fixed-base Em4.exe; the call site is verified byte-exact.
    match unsafe { redirect_call(BACKGROUND_CALL_SITE, BACKGROUND_ORIGINAL, background_pause as *const () as usize) } {
        PatchResult::Applied | PatchResult::AlreadyApplied => info!("Randloses Vollbild: schnelles Alt+Tab vorbereitet"),
        PatchResult::Mismatch(found) => {
            warn!("Randloses Vollbild: unbekannte Em4.exe-Version ({found:02X?}) – Alt+Tab bleibt wie im Original")
        }
        PatchResult::Failed => warn!("Randloses Vollbild: Patch fehlgeschlagen"),
    }
}

fn is_main(hwnd: HWND) -> bool {
    !hwnd.is_null() && hwnd as usize == MAIN_HWND.load(Ordering::Relaxed)
}

fn desktop_size() -> (u32, u32) {
    let d = DESKTOP.load(Ordering::Relaxed);
    if d != 0 {
        return (d >> 16, d & 0xFFFF);
    }
    let mut dm = [0u8; logic::DEVMODEA_SIZE];
    dm[36..38].copy_from_slice(&(logic::DEVMODEA_SIZE as u16).to_le_bytes());
    // SAFETY: buffer is a zeroed DEVMODEA with dmSize set.
    let ok = unsafe { EnumDisplaySettingsA(core::ptr::null(), 0xFFFF_FFFF, dm.as_mut_ptr()) } != 0;
    let (w, h) = if ok { logic::devmode_size(&dm) } else { (0, 0) };
    if w != 0 {
        DESKTOP.store((w << 16) | (h & 0xFFFF), Ordering::Relaxed);
    }
    (w, h)
}

unsafe fn class_name(class: *const u8) -> Option<String> {
    // Atoms are passed as small integers instead of pointers.
    if (class as usize) < 0x10000 {
        return None;
    }
    let mut n = 0;
    while n < 64 && *class.add(n) != 0 {
        n += 1;
    }
    Some(String::from_utf8_lossy(core::slice::from_raw_parts(class, n)).into_owned())
}

unsafe extern "system" fn hk_create_window_ex_a(
    ex_style: u32,
    class: *const u8,
    title: *const u8,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: HWND,
    menu: *mut c_void,
    instance: *mut c_void,
    param: *mut c_void,
) -> HWND {
    let real: CreateWindowExAFn = core::mem::transmute(ORIG_CREATE_WINDOW.load(Ordering::Acquire));
    let game_window = parent.is_null() && class_name(class).is_some_and(|c| logic::is_game_class(&c));
    let mut ex = ex_style;
    if game_window {
        // The size here is not final (Em4.exe creates e.g. 1920x987 and vBase71
        // resizes later); the mode request in ChangeDisplaySettingsA decides.
        desktop_size();
        ACTIVE.store(true, Ordering::Relaxed);
        ex = logic::strip_topmost(ex_style);
    }
    let hwnd = real(ex, class, title, style, x, y, w, h, parent, menu, instance, param);
    if game_window && !hwnd.is_null() {
        MAIN_HWND.store(hwnd as usize, Ordering::Relaxed);
    }
    hwnd
}

unsafe extern "system" fn hk_set_window_pos(hwnd: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> BOOL {
    let real: SetWindowPosFn = core::mem::transmute(ORIG_SET_WINDOW_POS.load(Ordering::Acquire));
    let after = if ACTIVE.load(Ordering::Relaxed) && is_main(hwnd) {
        logic::insert_after(after as isize) as HWND
    } else {
        after
    };
    real(hwnd, after, x, y, cx, cy, flags)
}

unsafe extern "system" fn hk_show_window(hwnd: HWND, cmd: i32) -> BOOL {
    let real: ShowWindowFn = core::mem::transmute(ORIG_SHOW_WINDOW.load(Ordering::Acquire));
    if FAKED.load(Ordering::Relaxed) && is_main(hwnd) && logic::is_minimize(cmd) {
        if !LOGGED_MIN.swap(true, Ordering::Relaxed) {
            info!("Randloses Vollbild: Minimieren des Spielfensters verhindert");
        }
        return 1;
    }
    real(hwnd, cmd)
}

unsafe extern "system" fn hk_change_display_settings_a(dm: *mut u8, flags: u32) -> i32 {
    let real: CdsFn = core::mem::transmute(ORIG_CDS.load(Ordering::Acquire));
    if ACTIVE.load(Ordering::Relaxed) {
        let requested = if dm.is_null() {
            None
        } else {
            Some(logic::devmode_size(core::slice::from_raw_parts(dm, logic::DEVMODEA_SIZE)))
        };
        let desk = desktop_size();
        match logic::cds_decision(requested, desk, FAKED.load(Ordering::Relaxed)) {
            logic::Cds::Skip => {
                if requested.is_some() {
                    FAKED.store(true, Ordering::Relaxed);
                    if !LOGGED_CDS.swap(true, Ordering::Relaxed) {
                        info!("Randloses Vollbild aktiv ({}x{}): Auflösungswechsel übersprungen", desk.0, desk.1);
                    }
                }
                return 0; // DISP_CHANGE_SUCCESSFUL
            }
            logic::Cds::PassThrough if requested.is_some() => {
                // A real mode change: from now on the original behaviour applies.
                ACTIVE.store(false, Ordering::Relaxed);
                FAKED.store(false, Ordering::Relaxed);
                let (w, h) = requested.unwrap_or_default();
                warn!(
                    "Randloses Vollbild nicht möglich: Spielauflösung {w}x{h} ≠ Desktop {}x{} – exklusives Vollbild bleibt",
                    desk.0, desk.1
                );
            }
            logic::Cds::PassThrough => {}
        }
    }
    real(dm, flags)
}

unsafe extern "system" fn hk_clip_cursor(rect: *const Rect) -> BOOL {
    let real: ClipCursorFn = core::mem::transmute(ORIG_CLIP.load(Ordering::Acquire));
    // Never trap the mouse while another window is in front.
    if FAKED.load(Ordering::Relaxed) && !rect.is_null() && !is_main(GetForegroundWindow()) {
        return real(core::ptr::null());
    }
    real(rect)
}

unsafe extern "system" fn hk_system_parameters_info_a(action: u32, ui: u32, pv: *mut c_void, win_ini: u32) -> BOOL {
    let real: SpiFn = core::mem::transmute(ORIG_SPI.load(Ordering::Acquire));
    if action == logic::SPI_SETSCREENSAVERRUNNING {
        if !LOGGED_SPI.swap(true, Ordering::Relaxed) {
            info!("Systemtasten bleiben frei (Druck-Taste, Windows-Taste)");
        }
        return 1;
    }
    real(action, ui, pv, win_ini)
}

/// Replacement for `0x5CF390`, called from the main loop while the
/// fullscreen window is inactive (cdecl, one argument).
extern "C" fn background_pause(arg: usize) {
    if !FAKED.load(Ordering::Relaxed) {
        // SAFETY: original routine, same calling convention and argument.
        let orig: extern "C" fn(usize) = unsafe { core::mem::transmute(BACKGROUND_ORIGINAL) };
        return orig(arg);
    }
    if !CFG_PAUSE.load(Ordering::Relaxed) {
        return; // keep running in the background
    }
    // SAFETY: Em4.exe functions with the signatures used by the original routine.
    unsafe {
        let get_screen: extern "C" fn() -> *const u8 = core::mem::transmute(GET_SCREEN);
        let get_input: extern "C" fn() -> usize = core::mem::transmute(GET_INPUT);
        let suspend: extern "thiscall" fn(usize) = core::mem::transmute(INPUT_SUSPEND);
        let resume: extern "thiscall" fn(usize) = core::mem::transmute(INPUT_RESUME);

        let screen = get_screen();
        if screen.is_null() {
            return;
        }
        let hwnd = *(screen.add(0x18) as *const HWND);
        let active = || *screen.add(0x25) != 0;
        if PAUSES.fetch_add(1, Ordering::Relaxed) == 0 {
            info!("Randloses Vollbild: Spiel pausiert im Hintergrund (ohne Minimieren)");
        }
        let clip_ptr = ORIG_CLIP.load(Ordering::Acquire);
        if clip_ptr != 0 {
            let clip: ClipCursorFn = core::mem::transmute(clip_ptr);
            clip(core::ptr::null());
        }
        suspend(get_input());
        let mut msg: Msg = core::mem::zeroed();
        loop {
            while PeekMessageA(&mut msg, hwnd, 0, 0, 1) != 0 {
                TranslateMessage(&msg);
                DispatchMessageA(&msg);
            }
            // WM_QUIT is not bound to the window: leave it for the game's loop.
            if PeekMessageA(&mut msg, core::ptr::null_mut(), 0x12, 0x12, 0) != 0 {
                break;
            }
            // Closed from the taskbar etc.: let the main loop quit.
            let quit = *(KEEP_RUNNING as *const u8) == 0;
            if quit || active() || IsWindow(hwnd) == 0 {
                break;
            }
            em4r_core::ffi::Sleep(15);
        }
        resume(get_input());
    }
}
