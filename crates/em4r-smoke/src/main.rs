//! Smoke test for the built proxy, without the game:
//!   em4r-smoke <path\to\dinput8.dll>
//! Copies the DLL into a fresh temp folder, loads it and verifies that it
//! creates its config and log and forwards DirectInput8Create to Windows.

use core::ffi::c_void;
use em4r_core::ffi::*;
use em4r_core::util::wide;
use std::process::ExitCode;

type DirectInput8CreateFn =
    unsafe extern "system" fn(HMODULE, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

// IID_IDirectInput8A {BF798030-483A-4DA2-AA99-5D64ED369700}
#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const IID_IDIRECTINPUT8A: Guid = Guid(0xBF798030, 0x483A, 0x4DA2, [0xAA, 0x99, 0x5D, 0x64, 0xED, 0x36, 0x97, 0x00]);

fn check(ok: bool, what: &str) -> bool {
    println!("[{}] {what}", if ok { " OK " } else { "FAIL" });
    ok
}

fn main() -> ExitCode {
    let Some(src) = std::env::args().nth(1) else {
        eprintln!("Aufruf: em4r-smoke <dinput8.dll>");
        return ExitCode::from(2);
    };
    let dir = std::env::temp_dir().join(format!("em4r_smoke_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let dll = dir.join("dinput8.dll");
    std::fs::copy(&src, &dll).expect("copy dll");

    // Same as the shipped defaults, but with the profiler switched on.
    // (Independent of line endings: switch the first `enabled` after [profiler].)
    let base = em4r_core::config::DEFAULT_TOML;
    let cfg = match base.find("[profiler]") {
        Some(p) => {
            let (head, tail) = base.split_at(p);
            format!("{head}{}", tail.replacen("enabled = false", "enabled = true", 1))
        }
        None => base.to_string(),
    };
    let profiler_on = em4r_core::Config::parse(&cfg).map(|c| c.profiler.enabled).unwrap_or(false);
    std::fs::write(dir.join("em4revive.toml"), cfg).expect("write toml");

    let mut all = true;
    all &= check(profiler_on, "Test-Config mit eingeschaltetem Profiler erzeugt");
    unsafe {
        let m = LoadLibraryW(wide(&dll).as_ptr());
        all &= check(!m.is_null(), "DLL lädt (DllMain läuft ohne Absturz)");
        if m.is_null() {
            return ExitCode::FAILURE;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let log = std::fs::read_to_string(dir.join("em4revive.log")).unwrap_or_default();
        all &= check(log.contains("EM4 Revive"), "em4revive.log wird geschrieben");
        all &= check(!log.contains("WARN  | Config"), "Config ohne Warnungen gelesen");
        all &= check(log.contains("Profiler aktiv"), "Profiler-Thread läuft");

        let p = GetProcAddress(m, b"DirectInput8Create\0".as_ptr());
        all &= check(!p.is_null(), "Export 'DirectInput8Create' (undekoriert) vorhanden");
        if !p.is_null() {
            let create: DirectInput8CreateFn = core::mem::transmute(p);
            let mut out: *mut c_void = core::ptr::null_mut();
            let hr = create(
                GetModuleHandleW(core::ptr::null()),
                0x0800,
                &IID_IDIRECTINPUT8A as *const Guid as *const c_void,
                &mut out,
                core::ptr::null_mut(),
            );
            all &= check(hr == 0 && !out.is_null(), &format!("DirectInput8Create -> HRESULT {hr:#010x}"));
            if !out.is_null() {
                // IUnknown::Release is vtable slot 2.
                let vtbl = *(out as *const *const usize);
                let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vtbl.add(2));
                release(out);
            }
        }
        println!("\n--- em4revive.log ---\n{log}");
    }
    let _ = std::fs::remove_dir_all(&dir); // may fail while the DLL is loaded; harmless
    if all { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
