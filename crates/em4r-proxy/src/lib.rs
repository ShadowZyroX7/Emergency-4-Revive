//! EM4 Revive loader.
//!
//! Built as `dinput8.dll` and placed next to `Em4.exe`. Windows loads it
//! instead of the system DLL because the game imports DINPUT8. We forward
//! `DirectInput8Create` to the real system DLL and, while loading, redirect
//! a few graphics imports of the game's modules to our improved versions.

use core::ffi::c_void;
use em4r_core::ffi::*;
use em4r_core::hook::{self, IatHook};
use em4r_core::util::{module_path, wide};
use em4r_core::{error, iat_hook, info, Config};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Modules in the game folder we must never patch (lower-case names).
/// `opengl32.dll` there is ReShade – it has to see the real calls.
const EXCLUDED_MODULES: &[&str] = &["opengl32.dll", "dinput8.dll", "dbghelp.dll"];

struct Context {
    game_dir: PathBuf,
    own: usize,
    hooks: Vec<IatHook>,
}

static CONTEXT: OnceLock<Context> = OnceLock::new();
static PATCH_LOCK: Mutex<()> = Mutex::new(());

// ---- DllMain ------------------------------------------------------------

#[no_mangle]
pub unsafe extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        DisableThreadLibraryCalls(module);
        // Never let a bug in the mod take the game down at startup.
        if std::panic::catch_unwind(|| init(module)).is_err() {
            error!("Interner Fehler bei der Initialisierung – Mod deaktiviert");
        }
    }
    TRUE
}

fn init(module: HMODULE) {
    let Some(dll_path) = module_path(module) else { return };
    let Some(game_dir) = dll_path.parent().map(PathBuf::from) else { return };

    let (cfg, cfg_error) = Config::load_or_create(&game_dir.join("em4revive.toml"));
    let log_ok = em4r_core::log::init(&game_dir.join("em4revive.log"), cfg.general.debug_log).is_ok();
    info!("EM4 Revive {VERSION} geladen in {}", module_path(core::ptr::null_mut()).unwrap_or_default().display());
    if let Some(e) = cfg_error {
        em4r_core::warn!("{e}");
    }
    if !log_ok {
        // Without a log we still run, but nobody could diagnose problems.
        em4r_core::log::defer_warning("Log-Datei konnte nicht angelegt werden".into());
    }
    if !cfg.general.enabled {
        info!("Mod ist in em4revive.toml deaktiviert – nur DirectInput wird durchgereicht");
        return;
    }
    em4r_core::config::set_global(cfg.clone());

    let mut hooks: Vec<IatHook> = Vec::new();
    if cfg.crash.minidump {
        em4r_core::crash::install(game_dir.clone());
        hooks.push(iat_hook!(
            "kernel32.dll",
            "SetUnhandledExceptionFilter",
            em4r_core::crash::hk_set_unhandled_exception_filter,
            ORIG_SET_UEF
        ));
    }
    if cfg.game.skip_intro {
        apply_exe_patch(&em4r_core::patch::SKIP_INTRO);
    }
    if cfg.perf.dpi_aware {
        // SAFETY: plain Win32 call.
        unsafe { SetProcessDPIAware() };
    }
    if cfg.perf.high_res_timer {
        // SAFETY: plain Win32 call; Windows resets it at process exit.
        unsafe { timeBeginPeriod(1) };
    }

    em4r_gl::configure(&cfg);
    hooks.extend(em4r_gl::hooks());
    if em4r_perf::wants_swap_hook(&cfg) {
        em4r_perf::configure(&cfg, &game_dir);
        hooks.extend(em4r_perf::hooks());
    }
    if cfg.profiler.enabled {
        if cfg.profiler.file_io {
            hooks.extend(em4r_prof::fileio::hooks());
        }
        // DllMain of a statically imported DLL runs on the main thread.
        em4r_prof::start(em4r_prof::Settings {
            // SAFETY: trivial Win32 call.
            main_thread_id: unsafe { GetCurrentThreadId() },
            game_dir: game_dir.clone(),
            sample_hz: cfg.profiler.sample_hz,
            report_interval_sec: cfg.profiler.report_interval_sec,
            file_io: cfg.profiler.file_io,
        });
    }
    // Functions resolved at runtime by name (e.g. Em4.exe's CreateFileW
    // pointer) are redirected as well.
    hooks.push(iat_hook!("kernel32.dll", "GetProcAddress", hk_get_proc_address, ORIG_GPA));
    // Modules loaded later (plugins) get patched as well.
    hooks.extend([
        iat_hook!("kernel32.dll", "LoadLibraryA", hk_load_library_a, ORIG_LLA),
        iat_hook!("kernel32.dll", "LoadLibraryW", hk_load_library_w, ORIG_LLW),
        iat_hook!("kernel32.dll", "LoadLibraryExA", hk_load_library_ex_a, ORIG_LLEXA),
        iat_hook!("kernel32.dll", "LoadLibraryExW", hk_load_library_ex_w, ORIG_LLEXW),
    ]);

    let _ = CONTEXT.set(Context { game_dir, own: module as usize, hooks });
    let installed = patch_game_modules();
    report(&installed);
}

/// Apply a verified code patch to Em4.exe (only if it is really Em4.exe).
fn apply_exe_patch(p: &em4r_core::patch::BytePatch) {
    use em4r_core::patch::PatchResult;
    let exe_name = module_path(core::ptr::null_mut())
        .map(|p| em4r_core::util::file_name_lower(&p))
        .unwrap_or_default();
    if exe_name != "em4.exe" {
        em4r_core::warn!("{}: Prozess ist nicht Em4.exe ({exe_name}) – übersprungen", p.name);
        return;
    }
    // SAFETY: we are in DllMain at process start; the game's main code has
    // not run yet, so nothing executes the patched bytes concurrently.
    let base = unsafe { GetModuleHandleW(core::ptr::null()) };
    match unsafe { em4r_core::patch::apply(base, p) } {
        PatchResult::Applied => info!("Patch aktiv: {}", p.name),
        PatchResult::AlreadyApplied => info!("Patch war schon aktiv: {}", p.name),
        PatchResult::Mismatch(found) => em4r_core::warn!(
            "{}: unbekannte Em4.exe-Version (gefunden {:02X?}, erwartet {:02X?}) – nicht angewendet",
            p.name, found, p.expected
        ),
        PatchResult::Failed => error!("{}: Speicher konnte nicht beschrieben werden", p.name),
    }
}

/// Patch every game module that is currently loaded.
fn patch_game_modules() -> Vec<hook::Installed> {
    let Some(ctx) = CONTEXT.get() else { return Vec::new() };
    let Ok(_guard) = PATCH_LOCK.lock() else { return Vec::new() };
    let mut all = Vec::new();
    for (m, name) in hook::game_modules(&ctx.game_dir, ctx.own as HMODULE, EXCLUDED_MODULES) {
        // SAFETY: `m` comes from the loader's module list.
        all.extend(unsafe { hook::install_into(m, &name, &ctx.hooks) });
    }
    all
}

fn report(installed: &[hook::Installed]) {
    let mut by_func: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for i in installed {
        let label = if i.delayed { format!("{} (delay-load)", i.module) } else { i.module.clone() };
        by_func.entry(i.func).or_default().push(label);
    }
    if by_func.is_empty() {
        em4r_core::warn!("Keine Hooks installiert – Spielmodule nicht gefunden?");
        return;
    }
    for (func, modules) in by_func {
        info!("Hook {func:<28} in {}", modules.join(", "));
    }
}

// ---- GetProcAddress: redirect runtime-resolved functions ------------------

static ORIG_GPA: AtomicUsize = AtomicUsize::new(0);
static GPA_LOGGED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Compare a NUL-terminated C string with `s`.
unsafe fn cstr_is(p: *const u8, s: &str) -> bool {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if *p.add(i) != c {
            return false;
        }
    }
    *p.add(b.len()) == 0
}

unsafe extern "system" fn hk_get_proc_address(module: HMODULE, name: *const u8) -> FARPROC {
    let f: unsafe extern "system" fn(HMODULE, *const u8) -> FARPROC =
        core::mem::transmute(ORIG_GPA.load(Ordering::Acquire));
    let result = f(module, name);
    // Ordinal lookups have a "name" below 0x10000.
    if result.is_null() || (name as usize) >> 16 == 0 {
        return result;
    }
    let Some(ctx) = CONTEXT.get() else { return result };
    for h in &ctx.hooks {
        // Only when we know the real function (so the detour can call it)
        // and the caller asked for exactly that function.
        if h.original() == result as usize && cstr_is(name, h.func) {
            if let Ok(mut logged) = GPA_LOGGED.lock() {
                if !logged.contains(&h.func) {
                    logged.push(h.func);
                    info!("GetProcAddress-Umleitung: {} (zur Laufzeit angefordert)", h.func);
                }
            }
            return h.detour as FARPROC;
        }
    }
    result
}

// ---- LoadLibrary hooks: patch modules loaded at runtime -------------------

static ORIG_SET_UEF: AtomicUsize = AtomicUsize::new(0);
static ORIG_LLA: AtomicUsize = AtomicUsize::new(0);
static ORIG_LLW: AtomicUsize = AtomicUsize::new(0);
static ORIG_LLEXA: AtomicUsize = AtomicUsize::new(0);
static ORIG_LLEXW: AtomicUsize = AtomicUsize::new(0);

fn after_load(result: HMODULE) -> HMODULE {
    if !result.is_null() {
        let installed = patch_game_modules();
        if !installed.is_empty() {
            report(&installed);
        }
    }
    result
}

unsafe extern "system" fn hk_load_library_a(name: *const u8) -> HMODULE {
    let f: unsafe extern "system" fn(*const u8) -> HMODULE = core::mem::transmute(ORIG_LLA.load(Ordering::Acquire));
    after_load(f(name))
}

unsafe extern "system" fn hk_load_library_w(name: *const u16) -> HMODULE {
    let f: unsafe extern "system" fn(*const u16) -> HMODULE = core::mem::transmute(ORIG_LLW.load(Ordering::Acquire));
    after_load(f(name))
}

unsafe extern "system" fn hk_load_library_ex_a(name: *const u8, file: HANDLE, flags: u32) -> HMODULE {
    let f: unsafe extern "system" fn(*const u8, HANDLE, u32) -> HMODULE =
        core::mem::transmute(ORIG_LLEXA.load(Ordering::Acquire));
    after_load(f(name, file, flags))
}

unsafe extern "system" fn hk_load_library_ex_w(name: *const u16, file: HANDLE, flags: u32) -> HMODULE {
    let f: unsafe extern "system" fn(*const u16, HANDLE, u32) -> HMODULE =
        core::mem::transmute(ORIG_LLEXW.load(Ordering::Acquire));
    after_load(f(name, file, flags))
}

// ---- DirectInput8 forwarding ----------------------------------------------

type DirectInput8CreateFn =
    unsafe extern "system" fn(HMODULE, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

const E_FAIL: i32 = 0x8000_4005u32 as i32;

/// The real `DirectInput8Create` from System32 (loaded on first use, not in
/// DllMain, to stay clear of the loader lock).
fn real_direct_input8_create() -> Option<DirectInput8CreateFn> {
    static REAL: OnceLock<usize> = OnceLock::new();
    let addr = *REAL.get_or_init(|| {
        let mut buf = vec![0u16; 512];
        // SAFETY: valid buffer.
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 || n >= buf.len() {
            error!("Systemordner nicht ermittelbar");
            return 0;
        }
        let mut path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        path.push("dinput8.dll");
        // SAFETY: NUL-terminated wide strings.
        unsafe {
            let m = LoadLibraryW(wide(&path).as_ptr());
            if m.is_null() {
                error!("Konnte {} nicht laden", path.display());
                return 0;
            }
            let p = GetProcAddress(m, b"DirectInput8Create\0".as_ptr());
            if p.is_null() {
                error!("DirectInput8Create fehlt in {}", path.display());
            }
            p as usize
        }
    });
    // SAFETY: address of the system export with this exact signature.
    (addr != 0).then(|| unsafe { core::mem::transmute::<usize, DirectInput8CreateFn>(addr) })
}

#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    hinst: HMODULE,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    // By now the game window exists and all engine DLLs are loaded: a good
    // moment for a second pass over imports that could not be resolved
    // during startup (cheap and idempotent).
    static SECOND_PASS: std::sync::Once = std::sync::Once::new();
    SECOND_PASS.call_once(|| {
        let installed = patch_game_modules();
        if !installed.is_empty() {
            report(&installed);
        }
    });
    match real_direct_input8_create() {
        Some(f) => f(hinst, version, riid, out, outer),
        None => E_FAIL,
    }
}
