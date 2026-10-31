//! Crash handler: writes a minidump and a log entry, then hands the crash
//! on to whatever handler the game installed.

use crate::ffi::*;
use crate::util::wide;
use core::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

static DUMP_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The handler the game wanted to install (we keep ours on top and chain).
static NEXT_FILTER: AtomicUsize = AtomicUsize::new(0);
static IN_HANDLER: AtomicBool = AtomicBool::new(false);

// MiniDumpWithDataSegs | MiniDumpWithHandleData | MiniDumpWithThreadInfo |
// MiniDumpWithIndirectlyReferencedMemory: small but useful dumps.
const DUMP_TYPE: u32 = 0x0001 | 0x0004 | 0x0040 | 0x1000;

pub fn install(dir: PathBuf) {
    let _ = DUMP_DIR.set(dir);
    // SAFETY: installing a valid filter function.
    let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    if let Some(p) = previous {
        NEXT_FILTER.store(p as usize, Ordering::Release);
    }
}

/// Replacement for the game's own `SetUnhandledExceptionFilter` calls:
/// remember the game's handler as the next in chain, keep ours installed.
pub unsafe extern "system" fn hk_set_unhandled_exception_filter(
    new: Option<TopLevelFilter>,
) -> Option<TopLevelFilter> {
    let prev = NEXT_FILTER.swap(new.map_or(0, |f| f as usize), Ordering::AcqRel);
    crate::debug!("Spiel setzt eigenen Crash-Handler {:#x} (wird verkettet)", new.map_or(0, |f| f as usize));
    // SAFETY: the value was stored from a valid `TopLevelFilter` or is 0.
    (prev != 0).then(|| core::mem::transmute::<usize, TopLevelFilter>(prev))
}

unsafe extern "system" fn filter(info: *mut c_void) -> i32 {
    if !IN_HANDLER.swap(true, Ordering::AcqRel) {
        write_dump(info);
    }
    let next = NEXT_FILTER.load(Ordering::Acquire);
    if next != 0 {
        let f: TopLevelFilter = core::mem::transmute(next);
        return f(info);
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe fn write_dump(info: *mut c_void) {
    let Some(dir) = DUMP_DIR.get() else { return };
    let code = if info.is_null() {
        0
    } else {
        // EXCEPTION_POINTERS { ExceptionRecord*, ContextRecord* };
        // EXCEPTION_RECORD starts with DWORD ExceptionCode, then flags,
        // record pointer and the faulting address.
        let rec = *(info as *const *const u32);
        if rec.is_null() { 0 } else { *rec }
    };
    let t = crate::util::local_time();
    let name = format!(
        "em4revive_crash_{:04}{:02}{:02}_{:02}{:02}{:02}.dmp",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let path = dir.join(&name);
    crate::error!("ABSTURZ: Exception-Code {:#010x}, schreibe {}", code, name);

    // Prefer the dbghelp.dll shipped with the game, fall back to Windows'.
    let mut dbghelp = LoadLibraryW(wide(dir.join("dbghelp.dll")).as_ptr());
    if dbghelp.is_null() {
        dbghelp = LoadLibraryW(wide("dbghelp.dll").as_ptr());
    }
    if dbghelp.is_null() {
        crate::error!("dbghelp.dll nicht gefunden, kein Dump");
        return;
    }
    let proc_ = GetProcAddress(dbghelp, b"MiniDumpWriteDump\0".as_ptr());
    if proc_.is_null() {
        crate::error!("MiniDumpWriteDump fehlt");
        return;
    }
    let write: MiniDumpWriteDumpFn = core::mem::transmute(proc_);
    let Ok(file) = std::fs::File::create(&path) else {
        crate::error!("Dump-Datei konnte nicht angelegt werden");
        return;
    };
    let exc = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: GetCurrentThreadId(),
        ExceptionPointers: info,
        ClientPointers: FALSE,
    };
    let ok = write(
        GetCurrentProcess(),
        GetCurrentProcessId(),
        file.as_raw_handle() as HANDLE,
        DUMP_TYPE,
        if info.is_null() { core::ptr::null() } else { &exc },
        core::ptr::null(),
        core::ptr::null(),
    );
    if ok != 0 {
        crate::error!("Dump geschrieben: {}", path.display());
    } else {
        crate::error!("MiniDumpWriteDump fehlgeschlagen");
    }
}
