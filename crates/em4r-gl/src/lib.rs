//! OpenGL / WGL hooks: pixel format (depth/stencil), texture filtering and
//! draw-call counting.

pub mod msaa;
pub mod pixelformat;
pub mod texture;

use em4r_core::hook::IatHook;
use em4r_core::iat_hook;
use std::sync::atomic::AtomicUsize;

/// Settings copied from the config at startup (read on hot paths).
#[derive(Debug, Clone, Copy)]
pub struct GlSettings {
    pub depth_bits: u8,
    pub stencil_bits: u8,
    pub msaa: u32,
    pub anisotropy: u32,
    pub force_trilinear: bool,
    pub count_draw_calls: bool,
}

static SETTINGS: std::sync::OnceLock<GlSettings> = std::sync::OnceLock::new();

pub fn configure(cfg: &em4r_core::Config) {
    let _ = SETTINGS.set(GlSettings {
        depth_bits: cfg.gl.force_depth_bits,
        stencil_bits: cfg.gl.force_stencil_bits,
        msaa: cfg.gl.msaa,
        anisotropy: cfg.gl.anisotropy,
        force_trilinear: cfg.gl.force_trilinear,
        count_draw_calls: cfg.gl.count_draw_calls,
    });
}

#[inline]
pub(crate) fn settings() -> &'static GlSettings {
    SETTINGS.get_or_init(|| GlSettings {
        depth_bits: 0,
        stencil_bits: 0,
        msaa: 0,
        anisotropy: 0,
        force_trilinear: false,
        count_draw_calls: false,
    })
}

// ---- draw-call counting -------------------------------------------------

static ORIG_DRAW_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
static ORIG_DRAW_ARRAYS: AtomicUsize = AtomicUsize::new(0);
static ORIG_BEGIN: AtomicUsize = AtomicUsize::new(0);

type DrawElements = unsafe extern "system" fn(u32, i32, u32, *const core::ffi::c_void);
type DrawArrays = unsafe extern "system" fn(u32, i32, i32);
type Begin = unsafe extern "system" fn(u32);

unsafe extern "system" fn hk_draw_elements(mode: u32, count: i32, ty: u32, idx: *const core::ffi::c_void) {
    em4r_core::counters::add_draw_call();
    let f: DrawElements = core::mem::transmute(ORIG_DRAW_ELEMENTS.load(std::sync::atomic::Ordering::Relaxed));
    f(mode, count, ty, idx)
}

unsafe extern "system" fn hk_draw_arrays(mode: u32, first: i32, count: i32) {
    em4r_core::counters::add_draw_call();
    let f: DrawArrays = core::mem::transmute(ORIG_DRAW_ARRAYS.load(std::sync::atomic::Ordering::Relaxed));
    f(mode, first, count)
}

unsafe extern "system" fn hk_begin(mode: u32) {
    em4r_core::counters::add_draw_call();
    let f: Begin = core::mem::transmute(ORIG_BEGIN.load(std::sync::atomic::Ordering::Relaxed));
    f(mode)
}

/// All hooks this crate wants, according to the settings.
pub fn hooks() -> Vec<IatHook> {
    let s = settings();
    let mut v = Vec::new();
    // Always installed: with depth/stencil overrides off they only pass the
    // call through and log the chosen format, which is useful diagnostics.
    v.extend(pixelformat::hooks());
    if s.anisotropy > 0 || s.force_trilinear {
        v.extend(texture::hooks());
    }
    if s.count_draw_calls {
        v.push(iat_hook!("opengl32.dll", "glDrawElements", hk_draw_elements, ORIG_DRAW_ELEMENTS));
        v.push(iat_hook!("opengl32.dll", "glDrawArrays", hk_draw_arrays, ORIG_DRAW_ARRAYS));
        v.push(iat_hook!("opengl32.dll", "glBegin", hk_begin, ORIG_BEGIN));
    }
    v
}
