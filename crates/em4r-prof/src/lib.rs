//! Sampling profiler + file-access statistics.
//!
//! A background thread briefly suspends the game's main thread about 1000
//! times per second and records where it is executing. Combined with the
//! file statistics this shows *where* EM4 loses time – while playing and
//! while loading – so optimisations target the real bottleneck.

pub mod aggregate;
pub mod fileio;
pub mod modmap;

use aggregate::Aggregate;
use em4r_core::counters::{phase_now, Phase};
use em4r_core::ffi::*;
use em4r_core::util::{qpc_now, qpc_to_ms};
use em4r_core::{info, warn};
use modmap::ModuleMap;
use std::fmt::Write;
use std::path::PathBuf;

/// Bytes of the suspended thread's stack copied per sample (for callers).
const STACK_COPY: usize = 1024;
const MODULE_REFRESH_MS: f64 = 2000.0;

pub struct Settings {
    pub main_thread_id: u32,
    pub game_dir: PathBuf,
    pub sample_hz: u32,
    pub report_interval_sec: u32,
    pub file_io: bool,
}

pub fn start(s: Settings) {
    if s.file_io {
        fileio::init();
    }
    let spawned = std::thread::Builder::new()
        .name("em4r-profiler".into())
        .stack_size(256 * 1024)
        .spawn(move || run(s));
    if let Err(e) = spawned {
        warn!("Profiler-Thread konnte nicht gestartet werden: {e}");
    }
}

#[cfg(target_arch = "x86")]
fn run(s: Settings) {
    // SAFETY: plain Win32 call; handle checked below.
    let thread = unsafe {
        OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, FALSE, s.main_thread_id)
    };
    if thread.is_null() {
        warn!("Profiler: Haupt-Thread {} nicht zugreifbar", s.main_thread_id);
        return;
    }
    info!("Profiler aktiv: {} Stichproben/s auf Thread {}", s.sample_hz, s.main_thread_id);

    let sleep_ms = (1000 / s.sample_hz).max(1);
    let report_path = s.game_dir.join("em4revive_profile.txt");
    let started = qpc_now();
    let mut cache = modmap::ExportCache::default();
    let mut mods = ModuleMap::snapshot(&s.game_dir, &mut cache);
    let mut last_refresh = started;
    let mut last_report = started;
    let mut agg = Aggregate::default();
    let mut stack = [0u32; STACK_COPY / 4];

    loop {
        // SAFETY: trivial Win32 call.
        unsafe { Sleep(sleep_ms) };

        // ---- critical section: main thread suspended -------------------
        // No allocation, no locks, no logging in here: the main thread may
        // hold the heap lock or one of our mutexes while suspended.
        let phase = phase_now();
        let mut ctx = CONTEXT::control();
        let mut copied = 0usize;
        // SAFETY: valid handle; buffers are pre-allocated.
        let ok = unsafe {
            if SuspendThread(thread) == u32::MAX {
                break; // thread gone (game exiting)
            }
            let ok = GetThreadContext(thread, &mut ctx) != 0;
            if ok {
                copied = copy_stack(ctx.esp() as usize, &mut stack);
            }
            ResumeThread(thread);
            ok
        };
        // ---- end of critical section ---------------------------------

        if ok {
            agg.add(phase, ctx.eip() as usize, &stack[..copied / 4], &mods, code_before);
        }

        let now = qpc_now();
        if qpc_to_ms(now - last_refresh) > MODULE_REFRESH_MS {
            mods = ModuleMap::snapshot(&s.game_dir, &mut cache);
            last_refresh = now;
        }
        if qpc_to_ms(now - last_report) > s.report_interval_sec as f64 * 1000.0 {
            last_report = now;
            write_report(&report_path, &agg, qpc_to_ms(now - started) / 1000.0, s.file_io);
        }
    }
    // SAFETY: handle from OpenThread.
    unsafe { CloseHandle(thread) };
}

#[cfg(not(target_arch = "x86"))]
fn run(_s: Settings) {
    warn!("Profiler nur für 32-bit (x86) implementiert");
}

/// Copy up to `buf.len()` dwords from the stack at `esp`, never reading past
/// the committed stack region. Returns bytes copied. Uses only syscalls, so
/// it is safe while the target thread is suspended.
#[cfg(target_arch = "x86")]
unsafe fn copy_stack(esp: usize, buf: &mut [u32]) -> usize {
    let mut mbi: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
    if VirtualQuery(esp as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) == 0 {
        return 0;
    }
    let region_end = mbi.BaseAddress as usize + mbi.RegionSize;
    let len = (region_end.saturating_sub(esp)).min(buf.len() * 4) & !3;
    let mut read = 0usize;
    if len == 0
        || ReadProcessMemory(GetCurrentProcess(), esp as *const _, buf.as_mut_ptr() as *mut _, len, &mut read) == 0
    {
        return 0;
    }
    read
}

/// The 7 code bytes before `addr`, read safely (unreadable -> None).
fn code_before(addr: usize) -> Option<[u8; 7]> {
    let mut buf = [0u8; 7];
    let mut read = 0usize;
    // SAFETY: ReadProcessMemory validates the address range itself.
    let ok = unsafe {
        ReadProcessMemory(GetCurrentProcess(), (addr - 7) as *const _, buf.as_mut_ptr() as *mut _, 7, &mut read)
    };
    (ok != 0 && read == 7).then_some(buf)
}

fn write_report(path: &std::path::Path, agg: &Aggregate, seconds: f64, file_io: bool) {
    let mut out = String::with_capacity(16 * 1024);
    let samples = agg.samples();
    let t = em4r_core::util::local_time();
    let _ = writeln!(out, "EM4 Revive – Leistungsprofil");
    let _ = writeln!(
        out,
        "Stand {:02}:{:02}:{:02}, Laufzeit {:.0} s, {} Stichproben (≈ {:.0}/s)",
        t.wHour, t.wMinute, t.wSecond, seconds, samples, samples as f64 / seconds.max(1.0)
    );
    let _ = writeln!(out, "Prozentangaben = Anteil der Zeit, die der Haupt-Thread dort verbracht hat.");
    for phase in Phase::ALL {
        agg.render_phase(phase, &mut out);
    }
    if file_io {
        fileio::render(&mut out);
    }
    let _ = writeln!(out, "\nHinweis: Aufrufer und Hotspots sind Adressen ohne Symbolnamen; sie dienen der weiteren Analyse.");
    if let Err(e) = std::fs::write(path, out) {
        warn!("Profil konnte nicht geschrieben werden: {e}");
    }
    let _ = std::fs::write(path.with_file_name("em4revive_profile_raw.csv"), agg.raw_csv());
}

#[cfg(all(test, target_arch = "x86"))]
mod tests {
    use super::*;

    #[test]
    fn copies_own_stack() {
        let marker: u32 = 0xC0FFEE11;
        let esp = &marker as *const u32 as usize;
        let mut buf = [0u32; 64];
        let n = unsafe { copy_stack(esp, &mut buf) };
        assert!(n >= 4);
        assert_eq!(buf[0], 0xC0FFEE11);
        core::hint::black_box(marker);
    }

    #[test]
    fn samples_a_busy_thread() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let stop = Arc::new(AtomicBool::new(false));
        let tid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (s2, t2) = (stop.clone(), tid.clone());
        let worker = std::thread::spawn(move || {
            t2.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
            let mut x = 0u64;
            while !s2.load(Ordering::Relaxed) {
                x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
        while tid.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        let h = unsafe {
            OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, FALSE, tid.load(Ordering::SeqCst))
        };
        assert!(!h.is_null());
        let mods = ModuleMap::snapshot(std::path::Path::new(r"C:\nowhere"), &mut modmap::ExportCache::default());
        let mut agg = Aggregate::default();
        let mut stack = [0u32; 256];
        for _ in 0..50 {
            unsafe {
                assert_ne!(SuspendThread(h), u32::MAX);
                let mut ctx = CONTEXT::control();
                assert!(GetThreadContext(h, &mut ctx) != 0);
                let n = copy_stack(ctx.esp() as usize, &mut stack);
                ResumeThread(h);
                agg.add(Phase::Playing, ctx.eip() as usize, &stack[..n / 4], &mods, code_before);
            }
        }
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        unsafe { CloseHandle(h) };
        assert_eq!(agg.samples(), 50);
        // The busy loop runs in the test executable itself.
        let exe = em4r_core::util::module_path(core::ptr::null_mut()).unwrap();
        let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
        let in_exe = agg.by_module.get(&(Phase::Playing, exe_name.clone())).copied().unwrap_or(0);
        assert!(in_exe >= 40, "nur {in_exe}/50 Stichproben in {exe_name}: {:?}", agg.by_module);
    }
}
