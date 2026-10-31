//! Direct start into Freeplay, requested by the launcher.
//!
//! Em4.exe (Deluxe, Steam) facts this relies on, all verified byte-for-byte
//! before anything is touched:
//!
//! * `0x477A60` is one main-loop iteration. At `0x477C10` it calls
//!   `ProcessMessages` (`0x73B390`, no arguments, returns `al` = keep running).
//!   We redirect exactly this call to [`main_loop_tick`]: it runs the original
//!   and afterwards – on the main thread, between two frames – starts the
//!   game once the main menu is up.
//! * `0x633980` is `StartGame(this = *(0xA93F20), int type, String* map)`
//!   (thiscall, callee cleans 8 bytes). The menu's Freeplay buttons call it
//!   with type 0 (endless) / 1 (challenge) and an empty map name, which
//!   selects `mod:Maps/freeplay.e4m`. Without a mod the button passes
//!   `dfreeplay.e4m` unless the byte at `0xB89704` is set – we mirror that.
//! * `0x72F330` / `0x72F0A0` are the engine's string constructor (from a C
//!   string) and destructor (both thiscall).
//!
//! If any signature differs (other game version), nothing is patched and
//! the player simply lands in the main menu.

use core::ffi::c_void;
use em4r_core::launch::LaunchRequest;
use em4r_core::patch::{self, PatchResult};
use em4r_core::{error, info, warn};
use std::path::Path;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

const MAIN_LOOP_CALL_SITE: usize = 0x477C10;
const PROCESS_MESSAGES: usize = 0x73B390;
const START_GAME: usize = 0x633980;
const STRING_CTOR: usize = 0x72F330;
const STRING_DTOR: usize = 0x72F0A0;
const GAME_SINGLETON: usize = 0xA93F20;
const NO_DELUXE_MAP_FLAG: usize = 0xB89704;

const SIG_START_GAME: &[u8] = &[0x6A, 0xFF, 0x64, 0xA1, 0x00, 0x00, 0x00, 0x00, 0x68, 0xEF, 0xFC, 0x99, 0x00, 0x50];
const SIG_STRING_CTOR: &[u8] = &[0x6A, 0xFF, 0x68, 0x55, 0xE2, 0x9A, 0x00, 0x64, 0xA1];
const SIG_STRING_DTOR: &[u8] = &[0x6A, 0xFF, 0x68, 0x1B, 0xE2, 0x9A, 0x00, 0x64, 0xA1];

/// The main menu must have been rendering this long before we start.
const MENU_READY_MS: f64 = 1500.0;
/// Give up if the menu never appears (e.g. an error dialog).
const GIVE_UP_AFTER_MS: f64 = 10.0 * 60.0 * 1000.0;

/// Pending game type (-1 = nothing pending).
static PENDING_TYPE: AtomicI32 = AtomicI32::new(-1);
static ARMED_AT_QPC: AtomicU64 = AtomicU64::new(0);
static MOD_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

type ProcessMessagesFn = unsafe extern "C" fn() -> u8;
type StartGameFn = unsafe extern "thiscall" fn(*mut c_void, i32, *mut u8);
type StringCtorFn = unsafe extern "thiscall" fn(*mut u8, *const u8) -> *mut u8;
type StringDtorFn = unsafe extern "thiscall" fn(*mut u8);

/// Read the launcher's request and, if it asks for an automatic start,
/// install the main-loop hook. Called from DllMain (before the game runs).
pub fn init(game_dir: &Path) {
    let req = match LaunchRequest::take(game_dir) {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            warn!("Startauftrag unlesbar, ignoriert: {e}");
            return;
        }
    };
    info!("Startauftrag vom Launcher: {}", req.mode.label());
    let Some(game_type) = req.mode.game_type() else { return };

    // Was the game started with -mod? (Decides the default map, see above.)
    let cmdline = std::env::args().collect::<Vec<_>>();
    MOD_ACTIVE.store(cmdline.iter().any(|a| a.eq_ignore_ascii_case("-mod")), Ordering::Relaxed);

    // SAFETY: fixed-base Em4.exe image; only reads.
    let signatures_ok = unsafe {
        patch::matches(START_GAME, SIG_START_GAME)
            && patch::matches(STRING_CTOR, SIG_STRING_CTOR)
            && patch::matches(STRING_DTOR, SIG_STRING_DTOR)
    };
    if !signatures_ok {
        warn!("Direktstart: unbekannte Em4.exe-Version – du landest im Hauptmenü");
        return;
    }
    // SAFETY: DllMain at process start, the main loop has not run yet.
    let r = unsafe { patch::redirect_call(MAIN_LOOP_CALL_SITE, PROCESS_MESSAGES, main_loop_tick as *const () as usize) };
    match r {
        PatchResult::Applied | PatchResult::AlreadyApplied => {
            PENDING_TYPE.store(game_type, Ordering::Release);
            ARMED_AT_QPC.store(em4r_core::util::qpc_now() as u64, Ordering::Relaxed);
            info!("Direktstart vorbereitet: {} (sobald das Hauptmenü bereit ist)", req.mode.label());
        }
        PatchResult::Mismatch(found) => {
            warn!("Direktstart: Hauptschleife unbekannt ({found:02X?}) – du landest im Hauptmenü")
        }
        PatchResult::Failed => error!("Direktstart: Patch fehlgeschlagen"),
    }
}

/// Replacement for the main loop's `ProcessMessages()` call.
unsafe extern "C" fn main_loop_tick() -> u8 {
    let original: ProcessMessagesFn = core::mem::transmute(PROCESS_MESSAGES);
    let keep_running = original();
    if keep_running != 0 && PENDING_TYPE.load(Ordering::Acquire) >= 0 {
        maybe_start();
    }
    keep_running
}

unsafe fn maybe_start() {
    let armed = ARMED_AT_QPC.load(Ordering::Relaxed) as i64;
    if em4r_core::util::qpc_to_ms(em4r_core::util::qpc_now() - armed) > GIVE_UP_AFTER_MS {
        PENDING_TYPE.store(-1, Ordering::Release);
        warn!("Direktstart abgebrochen: Hauptmenü kam nicht innerhalb von 10 Minuten");
        return;
    }
    // With -mod the first uninterrupted rendering is the mod's main menu.
    if em4r_core::counters::render_streak_ms() < MENU_READY_MS {
        return;
    }
    // Take the request (exactly once, even if StartGame re-enters the loop).
    let game_type = PENDING_TYPE.swap(-1, Ordering::AcqRel);
    if game_type < 0 {
        return;
    }
    let game = *(GAME_SINGLETON as *const *mut c_void);
    if game.is_null() {
        warn!("Direktstart: Spielobjekt fehlt – bleibe im Hauptmenü");
        return;
    }
    // Same map choice as the menu button.
    let use_deluxe_map = !MOD_ACTIVE.load(Ordering::Relaxed) && *(NO_DELUXE_MAP_FLAG as *const u8) == 0;
    let map: &[u8] = if use_deluxe_map { b"dfreeplay.e4m\0" } else { b"\0" };
    info!(
        "Direktstart: StartGame(typ {game_type}, Karte \"{}\")",
        String::from_utf8_lossy(&map[..map.len() - 1])
    );

    let ctor: StringCtorFn = core::mem::transmute(STRING_CTOR);
    let dtor: StringDtorFn = core::mem::transmute(STRING_DTOR);
    let start: StartGameFn = core::mem::transmute(START_GAME);
    // Engine string object: vtable + 3 fields (16 bytes); 64 is generous.
    #[repr(C, align(8))]
    struct StrBuf([u8; 64]);
    let mut s = StrBuf([0; 64]);
    let t0 = em4r_core::util::qpc_now();
    ctor(s.0.as_mut_ptr(), map.as_ptr());
    // StartGame loads the whole map before it returns.
    start(game, game_type, s.0.as_mut_ptr());
    dtor(s.0.as_mut_ptr());
    let secs = em4r_core::util::qpc_to_ms(em4r_core::util::qpc_now() - t0) / 1000.0;
    info!("Direktstart abgeschlossen: Karte in {secs:.0} s geladen");
}

/// Whether a direct start is waiting (the frame hook must stay active).
pub fn pending() -> bool {
    PENDING_TYPE.load(Ordering::Acquire) >= 0
}
