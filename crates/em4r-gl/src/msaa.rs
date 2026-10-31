//! Multisample anti-aliasing (MSAA) via the window's pixel format.
//!
//! Legacy OpenGL gets MSAA by choosing a pixel format with sample buffers.
//! Such formats are only reachable through `wglChoosePixelFormatARB`, and
//! that function can only be obtained while *some* GL context is current.
//! So, once, we create a hidden helper window with a plain context, fetch
//! the function and keep the pointer (ICD entry points stay valid). Then we
//! pick the MSAA format for the game's window; the game sets it itself.
//! GL_MULTISAMPLE is enabled by default when the format has samples.

use em4r_core::ffi::*;
use em4r_core::util::{loaded_proc, wide};
use em4r_core::{info, warn};
use std::sync::OnceLock;

const WGL_DRAW_TO_WINDOW_ARB: i32 = 0x2001;
const WGL_ACCELERATION_ARB: i32 = 0x2003;
const WGL_SUPPORT_OPENGL_ARB: i32 = 0x2010;
const WGL_DOUBLE_BUFFER_ARB: i32 = 0x2011;
const WGL_PIXEL_TYPE_ARB: i32 = 0x2013;
const WGL_COLOR_BITS_ARB: i32 = 0x2014;
const WGL_DEPTH_BITS_ARB: i32 = 0x2022;
const WGL_STENCIL_BITS_ARB: i32 = 0x2023;
const WGL_FULL_ACCELERATION_ARB: i32 = 0x2027;
const WGL_TYPE_RGBA_ARB: i32 = 0x202B;
const WGL_SAMPLE_BUFFERS_ARB: i32 = 0x2041;
const WGL_SAMPLES_ARB: i32 = 0x2042;

type ChooseArbFn = unsafe extern "system" fn(HDC, *const i32, *const f32, u32, *mut i32, *mut u32) -> BOOL;
type GetAttribArbFn = unsafe extern "system" fn(HDC, i32, i32, u32, *const i32, *mut i32) -> BOOL;
type CreateCtxFn = unsafe extern "system" fn(HDC) -> HGLRC;
type MakeCurrentFn = unsafe extern "system" fn(HDC, HGLRC) -> BOOL;
type DeleteCtxFn = unsafe extern "system" fn(HGLRC) -> BOOL;
type GetProcFn = unsafe extern "system" fn(*const u8) -> usize;
type GetCurrentFn = unsafe extern "system" fn() -> *mut core::ffi::c_void;

/// (wglChoosePixelFormatARB, wglGetPixelFormatAttribivARB) or (0, 0).
static ARB: OnceLock<(usize, usize)> = OnceLock::new();

/// Attribute list for wglChoosePixelFormatARB matching the game's request.
pub fn attribs(req: &PIXELFORMATDESCRIPTOR, depth: u8, stencil: u8, samples: u32) -> Vec<i32> {
    let mut a = vec![
        WGL_DRAW_TO_WINDOW_ARB, 1,
        WGL_SUPPORT_OPENGL_ARB, 1,
        WGL_ACCELERATION_ARB, WGL_FULL_ACCELERATION_ARB,
        WGL_PIXEL_TYPE_ARB, WGL_TYPE_RGBA_ARB,
        WGL_COLOR_BITS_ARB, req.cColorBits.max(24) as i32,
        WGL_DEPTH_BITS_ARB, req.cDepthBits.max(depth) as i32,
        WGL_STENCIL_BITS_ARB, req.cStencilBits.max(stencil) as i32,
        WGL_SAMPLE_BUFFERS_ARB, 1,
        WGL_SAMPLES_ARB, samples as i32,
    ];
    if req.dwFlags & PFD_DOUBLEBUFFER != 0 {
        a.extend([WGL_DOUBLE_BUFFER_ARB, 1]);
    }
    a.push(0);
    a
}

/// Sample counts to try, best first (e.g. 8 -> [8, 4, 2]).
pub fn fallback_chain(wanted: u32) -> Vec<u32> {
    [8u32, 4, 2].into_iter().filter(|&s| s <= wanted).collect()
}

fn gl(name: &str) -> Option<usize> {
    loaded_proc("opengl32.dll", name)
}

/// Create a throw-away context once and fetch the ARB entry points.
unsafe fn arb_functions() -> (usize, usize) {
    *ARB.get_or_init(|| {
        let (Some(create), Some(make_current), Some(delete), Some(get_proc), Some(cur_ctx), Some(cur_dc)) = (
            gl("wglCreateContext"),
            gl("wglMakeCurrent"),
            gl("wglDeleteContext"),
            gl("wglGetProcAddress"),
            gl("wglGetCurrentContext"),
            gl("wglGetCurrentDC"),
        ) else {
            warn!("MSAA: OpenGL-Funktionen nicht gefunden");
            return (0, 0);
        };
        let (create, make_current, delete, get_proc) = (
            core::mem::transmute::<usize, CreateCtxFn>(create),
            core::mem::transmute::<usize, MakeCurrentFn>(make_current),
            core::mem::transmute::<usize, DeleteCtxFn>(delete),
            core::mem::transmute::<usize, GetProcFn>(get_proc),
        );
        let prev_ctx = core::mem::transmute::<usize, GetCurrentFn>(cur_ctx)();
        let prev_dc = core::mem::transmute::<usize, GetCurrentFn>(cur_dc)();

        let class = wide("STATIC");
        let hwnd = CreateWindowExW(
            0, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 1, 1,
            core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(),
        );
        if hwnd.is_null() {
            warn!("MSAA: Hilfsfenster konnte nicht erstellt werden");
            return (0, 0);
        }
        let dc = GetDC(hwnd);
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: PFD_SIZE,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            cColorBits: 32,
            cDepthBits: 24,
            ..Default::default()
        };
        let mut result = (0, 0);
        let fmt = ChoosePixelFormat(dc, &pfd);
        if fmt != 0 && SetPixelFormat(dc, fmt, &pfd) != 0 {
            let ctx = create(dc);
            if !ctx.is_null() {
                if make_current(dc, ctx) != 0 {
                    result = (
                        get_proc(b"wglChoosePixelFormatARB\0".as_ptr()),
                        get_proc(b"wglGetPixelFormatAttribivARB\0".as_ptr()),
                    );
                }
                make_current(core::ptr::null_mut(), core::ptr::null_mut());
                delete(ctx);
            }
        }
        // Restore whatever was current before (normally nothing).
        if !prev_ctx.is_null() {
            make_current(prev_dc, prev_ctx);
        }
        ReleaseDC(hwnd, dc);
        DestroyWindow(hwnd);
        if result.0 == 0 {
            warn!("MSAA: wglChoosePixelFormatARB nicht verfügbar");
        }
        result
    })
}

/// Pick an MSAA pixel format for `hdc`. Returns (format, samples).
///
/// # Safety
/// `hdc` must be a valid device context of a window without a pixel format.
pub unsafe fn choose(hdc: HDC, req: &PIXELFORMATDESCRIPTOR, depth: u8, stencil: u8, wanted: u32) -> Option<(i32, u32)> {
    if wanted < 2 {
        return None;
    }
    let (choose_arb, get_attrib) = arb_functions();
    if choose_arb == 0 {
        return None;
    }
    let choose_arb: ChooseArbFn = core::mem::transmute(choose_arb);
    for samples in fallback_chain(wanted) {
        let a = attribs(req, depth, stencil, samples);
        let mut format = 0i32;
        let mut count = 0u32;
        if choose_arb(hdc, a.as_ptr(), core::ptr::null(), 1, &mut format, &mut count) != 0 && count > 0 && format > 0 {
            // Report what the driver actually gave us.
            let mut actual = samples as i32;
            if get_attrib != 0 {
                let get: GetAttribArbFn = core::mem::transmute(get_attrib);
                let q = [WGL_SAMPLES_ARB];
                get(hdc, format, 0, 1, q.as_ptr(), &mut actual);
            }
            if actual >= 2 {
                return Some((format, actual as u32));
            }
        }
    }
    info!("MSAA: kein passendes Multisample-Format gefunden – ohne Kantenglättung");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> PIXELFORMATDESCRIPTOR {
        PIXELFORMATDESCRIPTOR {
            nSize: PFD_SIZE,
            nVersion: 1,
            dwFlags: 0x25,
            cColorBits: 32,
            cDepthBits: 16,
            ..Default::default()
        }
    }

    fn get(a: &[i32], key: i32) -> Option<i32> {
        a.chunks(2).find(|c| c[0] == key).map(|c| c[1])
    }

    #[test]
    fn attribute_list() {
        let a = attribs(&req(), 24, 8, 4);
        assert_eq!(*a.last().unwrap(), 0);
        assert_eq!(get(&a, WGL_DEPTH_BITS_ARB), Some(24));
        assert_eq!(get(&a, WGL_STENCIL_BITS_ARB), Some(8));
        assert_eq!(get(&a, WGL_SAMPLES_ARB), Some(4));
        assert_eq!(get(&a, WGL_DOUBLE_BUFFER_ARB), Some(1));
        let mut single = req();
        single.dwFlags &= !PFD_DOUBLEBUFFER;
        assert_eq!(get(&attribs(&single, 24, 8, 2), WGL_DOUBLE_BUFFER_ARB), None);
    }

    #[test]
    fn fallbacks() {
        assert_eq!(fallback_chain(8), vec![8, 4, 2]);
        assert_eq!(fallback_chain(4), vec![4, 2]);
        assert_eq!(fallback_chain(2), vec![2]);
        assert!(fallback_chain(0).is_empty());
    }

    /// Real driver: a hidden test window must get an MSAA format.
    #[test]
    fn real_driver_offers_msaa() {
        // opengl32 must be loaded for loaded_proc().
        unsafe { LoadLibraryW(wide("opengl32.dll").as_ptr()) };
        let class = wide("STATIC");
        unsafe {
            let hwnd = CreateWindowExW(
                0, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 64, 64,
                core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(),
            );
            assert!(!hwnd.is_null());
            let dc = GetDC(hwnd);
            let r = choose(dc, &req(), 24, 8, 4);
            let (format, samples) = r.expect("Treiber bietet kein MSAA-Format");
            assert!(format > 0 && samples >= 2);
            let mut pfd = PIXELFORMATDESCRIPTOR::default();
            assert!(DescribePixelFormat(dc, format, PFD_SIZE as u32, &mut pfd) != 0);
            assert!(pfd.cDepthBits >= 24 && pfd.cStencilBits >= 8);
            ReleaseDC(hwnd, dc);
            DestroyWindow(hwnd);
        }
    }
}
