//! Depth/stencil fix.
//!
//! EM4 requests a pixel format with only 16 bit depth and no stencil, even
//! though `r_depthbits=24` is configured. 16 bit depth over the game's large
//! view distance causes z-fighting (flickering surfaces) and gives ReShade
//! poor depth data. We raise the request before the driver picks a format.
//!
//! Two layers, so it works whichever way the engine selects its format:
//! 1. `ChoosePixelFormat` / `wglChoosePixelFormat`: raise the request.
//! 2. `SetPixelFormat` / `wglSetPixelFormat`: if the index being set still
//!    has too little depth (e.g. the engine enumerated formats itself), pick
//!    an equivalent format that satisfies the request.

use em4r_core::ffi::*;
use em4r_core::hook::IatHook;
use em4r_core::{iat_hook, info, warn};
use std::sync::atomic::{AtomicUsize, Ordering};

type ChoosePf = unsafe extern "system" fn(HDC, *const PIXELFORMATDESCRIPTOR) -> i32;
type SetPf = unsafe extern "system" fn(HDC, i32, *const PIXELFORMATDESCRIPTOR) -> BOOL;

static ORIG_CHOOSE: AtomicUsize = AtomicUsize::new(0);
static ORIG_WGL_CHOOSE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET: AtomicUsize = AtomicUsize::new(0);
static ORIG_WGL_SET: AtomicUsize = AtomicUsize::new(0);

/// Flags a replacement format must share with the original one.
const REQUIRED_FLAGS: u32 = PFD_DOUBLEBUFFER | PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL;

pub fn hooks() -> Vec<IatHook> {
    vec![
        iat_hook!("gdi32.dll", "ChoosePixelFormat", hk_choose, ORIG_CHOOSE),
        iat_hook!("opengl32.dll", "wglChoosePixelFormat", hk_wgl_choose, ORIG_WGL_CHOOSE),
        iat_hook!("gdi32.dll", "SetPixelFormat", hk_set, ORIG_SET),
        iat_hook!("opengl32.dll", "wglSetPixelFormat", hk_wgl_set, ORIG_WGL_SET),
    ]
}

/// Raise depth/stencil in a request. Returns true if anything changed.
/// Values of 0 mean "leave as is"; we never lower what the game asked for.
pub fn raise_request(pfd: &mut PIXELFORMATDESCRIPTOR, depth: u8, stencil: u8) -> bool {
    let mut changed = false;
    if depth > 0 && pfd.cDepthBits < depth {
        pfd.cDepthBits = depth;
        changed = true;
    }
    if stencil > 0 && pfd.cStencilBits < stencil {
        pfd.cStencilBits = stencil;
        changed = true;
    }
    changed
}

/// Does an actual format satisfy the configured minimums?
pub fn satisfies(pfd: &PIXELFORMATDESCRIPTOR, depth: u8, stencil: u8) -> bool {
    pfd.cDepthBits >= depth && pfd.cStencilBits >= stencil
}

/// Is `candidate` an acceptable stand-in for `original`?
pub fn compatible(original: &PIXELFORMATDESCRIPTOR, candidate: &PIXELFORMATDESCRIPTOR) -> bool {
    candidate.iPixelType == original.iPixelType
        && candidate.cColorBits >= original.cColorBits
        && candidate.dwFlags & REQUIRED_FLAGS == original.dwFlags & REQUIRED_FLAGS
}

fn describe(hdc: HDC, format: i32) -> Option<PIXELFORMATDESCRIPTOR> {
    if format <= 0 {
        return None;
    }
    let mut pfd = PIXELFORMATDESCRIPTOR::default();
    // SAFETY: valid out pointer and size.
    let n = unsafe { DescribePixelFormat(hdc, format, PFD_SIZE as u32, &mut pfd) };
    (n != 0).then_some(pfd)
}

fn fmt_pfd(p: &PIXELFORMATDESCRIPTOR) -> String {
    format!(
        "Farbe {} / Alpha {} / Tiefe {} / Stencil {} / Flags {:#x}",
        p.cColorBits, p.cAlphaBits, p.cDepthBits, p.cStencilBits, p.dwFlags
    )
}

unsafe fn choose_common(orig: usize, name: &str, hdc: HDC, ppfd: *const PIXELFORMATDESCRIPTOR) -> i32 {
    let f: ChoosePf = core::mem::transmute(orig);
    let s = crate::settings();
    if ppfd.is_null() {
        return f(hdc, ppfd);
    }
    let requested = *ppfd;
    let mut raised = requested;
    let changed = raise_request(&mut raised, s.depth_bits, s.stencil_bits);
    let format = if changed { f(hdc, &raised) } else { 0 };

    if format != 0 {
        let got = describe(hdc, format);
        info!(
            "{name}: Spiel wollte [{}], angehoben auf Tiefe {} / Stencil {} -> Format {format} [{}]",
            fmt_pfd(&requested),
            raised.cDepthBits,
            raised.cStencilBits,
            got.as_ref().map(fmt_pfd).unwrap_or_else(|| "?".into())
        );
        return format;
    }
    if changed {
        warn!("{name}: Angehobene Anforderung abgelehnt, verwende Original");
    }
    let format = f(hdc, ppfd);
    info!(
        "{name}: [{}] -> Format {format} [{}]",
        fmt_pfd(&requested),
        describe(hdc, format).as_ref().map(fmt_pfd).unwrap_or_else(|| "?".into())
    );
    format
}

/// The real gdi32 `ChoosePixelFormat`, for re-selecting a format.
fn real_choose() -> Option<ChoosePf> {
    let orig = ORIG_CHOOSE.load(Ordering::Acquire);
    let addr = if orig != 0 { Some(orig) } else { em4r_core::util::loaded_proc("gdi32.dll", "ChoosePixelFormat") };
    // SAFETY: address of the gdi32 export with this signature.
    addr.map(|a| unsafe { core::mem::transmute::<usize, ChoosePf>(a) })
}

unsafe fn fix_format_index(name: &str, hdc: HDC, format: i32) -> i32 {
    let s = crate::settings();
    if s.depth_bits == 0 && s.stencil_bits == 0 {
        return format;
    }
    let Some(current) = describe(hdc, format) else { return format };
    if satisfies(&current, s.depth_bits, s.stencil_bits) {
        return format;
    }
    let mut want = current;
    want.nSize = PFD_SIZE;
    want.nVersion = 1;
    raise_request(&mut want, s.depth_bits, s.stencil_bits);
    let Some(choose) = real_choose() else { return format };
    let candidate = choose(hdc, &want);
    match describe(hdc, candidate) {
        Some(c) if satisfies(&c, s.depth_bits, s.stencil_bits) && compatible(&current, &c) => {
            info!("{name}: Format {format} [{}] ersetzt durch {candidate} [{}]", fmt_pfd(&current), fmt_pfd(&c));
            candidate
        }
        _ => {
            warn!("{name}: Kein passendes Format mit mehr Tiefe gefunden, behalte {format} [{}]", fmt_pfd(&current));
            format
        }
    }
}

unsafe extern "system" fn hk_choose(hdc: HDC, ppfd: *const PIXELFORMATDESCRIPTOR) -> i32 {
    choose_common(ORIG_CHOOSE.load(Ordering::Acquire), "ChoosePixelFormat", hdc, ppfd)
}

unsafe extern "system" fn hk_wgl_choose(hdc: HDC, ppfd: *const PIXELFORMATDESCRIPTOR) -> i32 {
    choose_common(ORIG_WGL_CHOOSE.load(Ordering::Acquire), "wglChoosePixelFormat", hdc, ppfd)
}

unsafe extern "system" fn hk_set(hdc: HDC, format: i32, ppfd: *const PIXELFORMATDESCRIPTOR) -> BOOL {
    let f: SetPf = core::mem::transmute(ORIG_SET.load(Ordering::Acquire));
    let format = fix_format_index("SetPixelFormat", hdc, format);
    f(hdc, format, ppfd)
}

unsafe extern "system" fn hk_wgl_set(hdc: HDC, format: i32, ppfd: *const PIXELFORMATDESCRIPTOR) -> BOOL {
    let f: SetPf = core::mem::transmute(ORIG_WGL_SET.load(Ordering::Acquire));
    let format = fix_format_index("wglSetPixelFormat", hdc, format);
    f(hdc, format, ppfd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn em4_request() -> PIXELFORMATDESCRIPTOR {
        // What EM4 asks for according to ReShade.log.
        PIXELFORMATDESCRIPTOR {
            nSize: PFD_SIZE,
            nVersion: 1,
            dwFlags: 0x25,
            cColorBits: 32,
            cDepthBits: 16,
            cStencilBits: 0,
            ..Default::default()
        }
    }

    #[test]
    fn raises_em4_request() {
        let mut p = em4_request();
        assert!(raise_request(&mut p, 24, 8));
        assert_eq!((p.cDepthBits, p.cStencilBits), (24, 8));
        assert_eq!(p.dwFlags, 0x25);
        assert_eq!(p.cColorBits, 32);
    }

    #[test]
    fn never_lowers_and_zero_means_untouched() {
        let mut p = em4_request();
        p.cDepthBits = 32;
        assert!(!raise_request(&mut p, 24, 0));
        assert_eq!((p.cDepthBits, p.cStencilBits), (32, 0));
        let mut q = em4_request();
        assert!(!raise_request(&mut q, 0, 0));
        assert_eq!(q, em4_request());
    }

    #[test]
    fn satisfaction_and_compatibility() {
        let orig = em4_request();
        let mut good = orig;
        good.cDepthBits = 24;
        good.cStencilBits = 8;
        assert!(!satisfies(&orig, 24, 8));
        assert!(satisfies(&good, 24, 8));
        assert!(compatible(&orig, &good));

        let mut single_buffered = good;
        single_buffered.dwFlags &= !PFD_DOUBLEBUFFER;
        assert!(!compatible(&orig, &single_buffered));

        let mut fewer_colors = good;
        fewer_colors.cColorBits = 16;
        assert!(!compatible(&orig, &fewer_colors));
    }
}
