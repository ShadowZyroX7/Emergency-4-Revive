//! One-frame GL call trace (developer diagnostics).
//!
//! Records, for a single in-game frame, the order of structural GL calls
//! (projection, viewport, clears, copies to textures, context switches) per
//! calling module plus the number of draw calls in between. Used to find
//! where the 3D world ends and bloom/UI begin, i.e. where post-processing
//! effects must be inserted.
//!
//! The hooked functions are not used by any other EM4 Revive feature, so
//! every module gets its own detours and originals.

use em4r_core::hook::IatHook;
use em4r_core::iat_hook;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

static ARMED: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static MATRIX_MODE: AtomicU32 = AtomicU32::new(0x1700);
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static OUT_PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

const GL_PROJECTION: u32 = 0x1701;

pub fn arm(out: std::path::PathBuf) {
    if let Ok(mut p) = OUT_PATH.lock() {
        *p = Some(out);
    }
    ARMED.store(true, Ordering::Release);
}

/// Called at every buffer swap. Starts recording at the swap that begins
/// the chosen frame and writes the file at the swap that ends it.
pub fn on_swap(in_game: bool) {
    if !ARMED.load(Ordering::Acquire) || DONE.load(Ordering::Acquire) {
        return;
    }
    if RECORDING.load(Ordering::Acquire) {
        RECORDING.store(false, Ordering::Release);
        DONE.store(true, Ordering::Release);
        write_file();
    } else if in_game {
        if let Ok(mut e) = EVENTS.lock() {
            e.clear();
            e.push(format!("--- Frame-Beginn (Draw-Calls zählen ab hier) ---"));
        }
        em4r_core::counters::take_draw_calls();
        RECORDING.store(true, Ordering::Release);
    }
}

fn event(module: &str, text: String) {
    if !RECORDING.load(Ordering::Relaxed) {
        return;
    }
    let draws = em4r_core::counters::DRAW_CALLS.load(Ordering::Relaxed);
    if let Ok(mut e) = EVENTS.lock() {
        if e.len() < 20_000 {
            e.push(format!("[{draws:>5} draws] {module:<8} {text}"));
        }
    }
}

fn write_file() {
    let Some(path) = OUT_PATH.lock().ok().and_then(|p| p.clone()) else { return };
    let mut out = String::new();
    if let Ok(e) = EVENTS.lock() {
        for l in e.iter() {
            let _ = writeln!(out, "{l}");
        }
        let _ = writeln!(out, "--- Frame-Ende: {} Draw-Calls ---", em4r_core::counters::DRAW_CALLS.load(Ordering::Relaxed));
    }
    let _ = std::fs::write(&path, out);
    em4r_core::info!("Frame-Trace geschrieben: {}", path.display());
}

fn proj_kind(m: &[f32; 16]) -> &'static str {
    // Column-major: m[11] = -1 and m[15] = 0 for perspective, 0/1 for ortho.
    if m[15] == 1.0 && m[11] == 0.0 {
        "ORTHO"
    } else if m[15] == 0.0 {
        "PERSPEKTIVE"
    } else {
        "andere"
    }
}

/// Generates per-module detours for the traced functions.
macro_rules! traced_module {
    ($modname:ident, $label:expr) => {
        pub mod $modname {
            use super::*;
            static O_FRUSTUM: AtomicUsize = AtomicUsize::new(0);
            static O_LOADMATRIX: AtomicUsize = AtomicUsize::new(0);
            static O_MATRIXMODE: AtomicUsize = AtomicUsize::new(0);
            static O_LOADIDENTITY: AtomicUsize = AtomicUsize::new(0);
            static O_VIEWPORT: AtomicUsize = AtomicUsize::new(0);
            static O_CLEAR: AtomicUsize = AtomicUsize::new(0);
            static O_COPYSUB: AtomicUsize = AtomicUsize::new(0);
            static O_COPY: AtomicUsize = AtomicUsize::new(0);
            static O_READPIXELS: AtomicUsize = AtomicUsize::new(0);
            static O_SCISSOR: AtomicUsize = AtomicUsize::new(0);
            static O_MAKECURRENT: AtomicUsize = AtomicUsize::new(0);
            static O_ENABLE: AtomicUsize = AtomicUsize::new(0);

            unsafe extern "system" fn frustum(l: f64, r: f64, b: f64, t: f64, n: f64, f: f64) {
                event($label, format!("glFrustum(near {n:.1}, far {f:.0})"));
                let o: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64) =
                    core::mem::transmute(O_FRUSTUM.load(Ordering::Relaxed));
                o(l, r, b, t, n, f)
            }
            unsafe extern "system" fn load_matrix(m: *const f32) {
                if MATRIX_MODE.load(Ordering::Relaxed) == GL_PROJECTION && !m.is_null() {
                    let mm = &*(m as *const [f32; 16]);
                    event($label, format!("Projektion glLoadMatrixf -> {}", proj_kind(mm)));
                }
                let o: unsafe extern "system" fn(*const f32) = core::mem::transmute(O_LOADMATRIX.load(Ordering::Relaxed));
                o(m)
            }
            unsafe extern "system" fn matrix_mode(mode: u32) {
                MATRIX_MODE.store(mode, Ordering::Relaxed);
                let o: unsafe extern "system" fn(u32) = core::mem::transmute(O_MATRIXMODE.load(Ordering::Relaxed));
                o(mode)
            }
            unsafe extern "system" fn load_identity() {
                if MATRIX_MODE.load(Ordering::Relaxed) == GL_PROJECTION {
                    event($label, "Projektion glLoadIdentity".into());
                }
                let o: unsafe extern "system" fn() = core::mem::transmute(O_LOADIDENTITY.load(Ordering::Relaxed));
                o()
            }
            unsafe extern "system" fn viewport(x: i32, y: i32, w: i32, h: i32) {
                event($label, format!("glViewport({x},{y},{w},{h})"));
                let o: unsafe extern "system" fn(i32, i32, i32, i32) =
                    core::mem::transmute(O_VIEWPORT.load(Ordering::Relaxed));
                o(x, y, w, h)
            }
            unsafe extern "system" fn clear(mask: u32) {
                let mut parts = Vec::new();
                if mask & 0x4000 != 0 { parts.push("Farbe"); }
                if mask & 0x100 != 0 { parts.push("Tiefe"); }
                if mask & 0x400 != 0 { parts.push("Stencil"); }
                event($label, format!("glClear({})", parts.join("+")));
                let o: unsafe extern "system" fn(u32) = core::mem::transmute(O_CLEAR.load(Ordering::Relaxed));
                o(mask)
            }
            unsafe extern "system" fn copy_sub(t: u32, lvl: i32, xo: i32, yo: i32, x: i32, y: i32, w: i32, h: i32) {
                event($label, format!("glCopyTexSubImage2D({w}x{h} von {x},{y})"));
                let o: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, i32, i32) =
                    core::mem::transmute(O_COPYSUB.load(Ordering::Relaxed));
                o(t, lvl, xo, yo, x, y, w, h)
            }
            unsafe extern "system" fn copy(t: u32, lvl: i32, fmt: u32, x: i32, y: i32, w: i32, h: i32, b: i32) {
                event($label, format!("glCopyTexImage2D({w}x{h} von {x},{y}, Format 0x{fmt:X})"));
                let o: unsafe extern "system" fn(u32, i32, u32, i32, i32, i32, i32, i32) =
                    core::mem::transmute(O_COPY.load(Ordering::Relaxed));
                o(t, lvl, fmt, x, y, w, h, b)
            }
            unsafe extern "system" fn read_pixels(x: i32, y: i32, w: i32, h: i32, f: u32, ty: u32, p: *mut u8) {
                event($label, format!("glReadPixels({w}x{h}, Format 0x{f:X})"));
                let o: unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut u8) =
                    core::mem::transmute(O_READPIXELS.load(Ordering::Relaxed));
                o(x, y, w, h, f, ty, p)
            }
            unsafe extern "system" fn scissor(x: i32, y: i32, w: i32, h: i32) {
                event($label, format!("glScissor({x},{y},{w},{h})"));
                let o: unsafe extern "system" fn(i32, i32, i32, i32) =
                    core::mem::transmute(O_SCISSOR.load(Ordering::Relaxed));
                o(x, y, w, h)
            }
            unsafe extern "system" fn make_current(dc: *mut core::ffi::c_void, rc: *mut core::ffi::c_void) -> i32 {
                event($label, format!("wglMakeCurrent(dc {:p}, rc {:p})", dc, rc));
                let o: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32 =
                    core::mem::transmute(O_MAKECURRENT.load(Ordering::Relaxed));
                o(dc, rc)
            }
            unsafe extern "system" fn enable(cap: u32) {
                // Only the interesting ones: depth test, blending, scissor.
                match cap {
                    0x0B71 => event($label, "glEnable(DEPTH_TEST)".into()),
                    0x0BE2 => event($label, "glEnable(BLEND)".into()),
                    0x0C11 => event($label, "glEnable(SCISSOR_TEST)".into()),
                    _ => {}
                }
                let o: unsafe extern "system" fn(u32) = core::mem::transmute(O_ENABLE.load(Ordering::Relaxed));
                o(cap)
            }

            pub fn hooks() -> Vec<IatHook> {
                vec![
                    iat_hook!("opengl32.dll", "glFrustum", frustum, O_FRUSTUM),
                    iat_hook!("opengl32.dll", "glLoadMatrixf", load_matrix, O_LOADMATRIX),
                    iat_hook!("opengl32.dll", "glMatrixMode", matrix_mode, O_MATRIXMODE),
                    iat_hook!("opengl32.dll", "glLoadIdentity", load_identity, O_LOADIDENTITY),
                    iat_hook!("opengl32.dll", "glViewport", viewport, O_VIEWPORT),
                    iat_hook!("opengl32.dll", "glClear", clear, O_CLEAR),
                    iat_hook!("opengl32.dll", "glCopyTexSubImage2D", copy_sub, O_COPYSUB),
                    iat_hook!("opengl32.dll", "glCopyTexImage2D", copy, O_COPY),
                    iat_hook!("opengl32.dll", "glReadPixels", read_pixels, O_READPIXELS),
                    iat_hook!("opengl32.dll", "glScissor", scissor, O_SCISSOR),
                    iat_hook!("opengl32.dll", "wglMakeCurrent", make_current, O_MAKECURRENT),
                    iat_hook!("opengl32.dll", "glEnable", enable, O_ENABLE),
                ]
            }
        }
    };
}

traced_module!(vision, "vision71");
traced_module!(em4, "Em4.exe");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_classification() {
        let mut ortho = [0.0f32; 16];
        ortho[0] = 1.0;
        ortho[5] = 1.0;
        ortho[10] = -1.0;
        ortho[15] = 1.0;
        assert_eq!(proj_kind(&ortho), "ORTHO");
        let mut persp = [0.0f32; 16];
        persp[11] = -1.0;
        assert_eq!(proj_kind(&persp), "PERSPEKTIVE");
    }
}
