//! Frame pacing and measurement, driven from the buffer-swap hook.

pub mod limiter;
pub mod stats;

use em4r_core::hook::IatHook;
use em4r_core::util::{process_memory, qpc_now, qpc_to_ms};
use em4r_core::{iat_hook, info, warn};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

type SwapBuffersFn = unsafe extern "system" fn(em4r_core::ffi::HDC) -> em4r_core::ffi::BOOL;
type WglGetProcAddress = unsafe extern "system" fn(*const u8) -> usize;
type WglSwapInterval = unsafe extern "system" fn(i32) -> em4r_core::ffi::BOOL;

static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);
static ORIG_WGL_SWAP: AtomicUsize = AtomicUsize::new(0);

/// Address-space warning threshold for a 32-bit LAA process (4 GB max).
const MEMORY_WARN_BYTES: usize = 3_400 * 1024 * 1024;

struct State {
    stats_enabled: bool,
    interval_ticks: i64,
    vsync: i32,
    vsync_applied: bool,
    limiter: Option<limiter::Limiter>,
    csv: Option<BufWriter<File>>,
    start: i64,
    last_frame: i64,
    interval_start: i64,
    frametimes: Vec<f32>,
    draws: Vec<u32>,
    intervals: u64,
    memory_warned: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn configure(cfg: &em4r_core::Config, game_dir: &Path) {
    let csv = if cfg.stats.enabled && cfg.stats.csv {
        open_csv(&game_dir.join("em4revive_stats.csv"))
    } else {
        None
    };
    let now = qpc_now();
    let state = State {
        stats_enabled: cfg.stats.enabled,
        interval_ticks: (cfg.stats.interval_sec as f64 * em4r_core::util::qpc_freq() as f64) as i64,
        vsync: cfg.perf.vsync,
        vsync_applied: false,
        limiter: limiter::Limiter::new(cfg.perf.frame_limit),
        csv,
        start: now,
        last_frame: 0,
        interval_start: 0,
        frametimes: Vec::with_capacity(512),
        draws: Vec::with_capacity(512),
        intervals: 0,
        memory_warned: false,
    };
    if cfg.perf.frame_limit > 0 {
        info!("FPS-Limit: {}", cfg.perf.frame_limit);
    }
    if let Ok(mut g) = STATE.lock() {
        *g = Some(state);
    }
}

fn open_csv(path: &Path) -> Option<BufWriter<File>> {
    match File::create(path) {
        Ok(f) => {
            let mut w = BufWriter::new(f);
            let _ = writeln!(
                w,
                "zeit_s;fps_avg;fps_1pct_low;frametime_max_ms;draw_calls_avg;ram_privat_mb;ram_working_set_mb;bildwechsel_wartezeit_ms"
            );
            Some(w)
        }
        Err(e) => {
            warn!("Konnte {} nicht anlegen: {e}", path.display());
            None
        }
    }
}

/// Whether any swap hook is needed at all.
pub fn wants_swap_hook(cfg: &em4r_core::Config) -> bool {
    // The profiler needs frame timestamps to tell loading from playing.
    cfg.stats.enabled || cfg.perf.frame_limit > 0 || cfg.perf.vsync >= 0 || cfg.profiler.enabled
}

pub fn hooks() -> Vec<IatHook> {
    vec![
        iat_hook!("gdi32.dll", "SwapBuffers", hk_swap_buffers, ORIG_SWAP),
        iat_hook!("opengl32.dll", "wglSwapBuffers", hk_wgl_swap_buffers, ORIG_WGL_SWAP),
    ]
}

/// Time spent inside the real SwapBuffers since the last stats interval.
/// High values mean the CPU waits for the GPU (or VSync) at frame end.
static SWAP_TICKS: AtomicU64 = AtomicU64::new(0);

unsafe fn timed_swap(orig: &AtomicUsize, hdc: em4r_core::ffi::HDC) -> em4r_core::ffi::BOOL {
    let f: SwapBuffersFn = core::mem::transmute(orig.load(Ordering::Acquire));
    let t0 = qpc_now();
    let r = f(hdc);
    SWAP_TICKS.fetch_add((qpc_now() - t0) as u64, Ordering::Relaxed);
    r
}

unsafe extern "system" fn hk_swap_buffers(hdc: em4r_core::ffi::HDC) -> em4r_core::ffi::BOOL {
    before_present();
    timed_swap(&ORIG_SWAP, hdc)
}

unsafe extern "system" fn hk_wgl_swap_buffers(hdc: em4r_core::ffi::HDC) -> em4r_core::ffi::BOOL {
    before_present();
    timed_swap(&ORIG_WGL_SWAP, hdc)
}

/// Called on the render thread right before every buffer swap.
fn before_present() {
    let now = qpc_now();
    em4r_core::counters::mark_present(now);
    let draws = em4r_core::counters::take_draw_calls();
    // try_lock: never stall the render thread on our own bookkeeping.
    let Ok(mut guard) = STATE.try_lock() else { return };
    let Some(st) = guard.as_mut() else { return };

    if !st.vsync_applied {
        st.vsync_applied = true;
        if st.vsync >= 0 {
            apply_vsync(st.vsync);
        }
    }

    if st.stats_enabled {
        if st.last_frame != 0 {
            st.frametimes.push(qpc_to_ms(now - st.last_frame) as f32);
            st.draws.push(draws);
        } else {
            st.interval_start = now;
        }
        st.last_frame = now;
        if now - st.interval_start >= st.interval_ticks {
            flush_interval(st, now);
            st.interval_start = now;
        }
    }

    // Frametimes are measured entry-to-entry, so this wait is correctly
    // included in the frame it delays.
    if let Some(l) = st.limiter.as_mut() {
        l.wait();
    }
}

fn flush_interval(st: &mut State, now: i64) {
    let summary = stats::summarize(&st.frametimes, &st.draws);
    let swap_ticks = SWAP_TICKS.swap(0, Ordering::Relaxed);
    st.frametimes.clear();
    st.draws.clear();
    let Some(s) = summary else { return };
    let swap_ms = qpc_to_ms(swap_ticks as i64) / s.frames.max(1) as f64;
    st.intervals += 1;
    let (private_mb, ws_mb) = process_memory()
        .map(|(p, w)| (p as f64 / 1048576.0, w as f64 / 1048576.0))
        .unwrap_or((0.0, 0.0));
    let t = qpc_to_ms(now - st.start) / 1000.0;
    if let Some(w) = st.csv.as_mut() {
        // German Excel expects ';' as separator and ',' as decimal mark.
        let line = format!(
            "{:.1};{:.1};{:.1};{:.2};{:.0};{:.0};{:.0};{:.2}",
            t, s.fps_avg, s.fps_1pct_low, s.frametime_max_ms, s.draws_avg, private_mb, ws_mb, swap_ms
        )
        .replace('.', ",");
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
    // A log line every ~10 intervals keeps the log readable.
    if st.intervals % 10 == 1 {
        let frame_ms = 1000.0 / s.fps_avg.max(0.001);
        info!(
            "FPS {:.1} (1%-Low {:.1}, max Frametime {:.1} ms) | Draw-Calls {:.0} | RAM {:.0} MB | Bildwechsel (ReShade + Warten auf GPU/VSync) {:.1} ms von {:.1} ms pro Bild",
            s.fps_avg, s.fps_1pct_low, s.frametime_max_ms, s.draws_avg, private_mb, swap_ms, frame_ms
        );
    }
    if !st.memory_warned && private_mb as usize * 1048576 > MEMORY_WARN_BYTES {
        st.memory_warned = true;
        warn!(
            "Speicherverbrauch {:.0} MB nähert sich der 4-GB-Grenze (32-bit). Absturzgefahr bei weiteren Mods/Karten!",
            private_mb
        );
    }
}

fn apply_vsync(interval: i32) {
    let Some(gpa) = em4r_core::util::loaded_proc("opengl32.dll", "wglGetProcAddress") else {
        warn!("VSync: wglGetProcAddress nicht gefunden");
        return;
    };
    // SAFETY: wglGetProcAddress signature; a GL context is current while
    // the game is presenting.
    unsafe {
        let gpa: WglGetProcAddress = core::mem::transmute(gpa);
        let p = gpa(b"wglSwapIntervalEXT\0".as_ptr());
        if p == 0 {
            warn!("VSync: wglSwapIntervalEXT nicht verfügbar");
            return;
        }
        let set: WglSwapInterval = core::mem::transmute(p);
        if set(interval) != 0 {
            info!("VSync gesetzt: Intervall {interval}");
        } else {
            warn!("VSync: wglSwapIntervalEXT({interval}) fehlgeschlagen");
        }
    }
}
