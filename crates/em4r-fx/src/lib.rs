//! EM4 Revive post-processing.
//!
//! Where: a frame trace showed that vision71 renders shadow maps (pbuffer),
//! then the 3D world with a perspective projection (glFrustum, near 100),
//! then switches the projection to identity for its 2D UI, after which
//! Em4.exe draws its own overlays. We run the effects right before the
//! first draw call after that switch – so they affect the 3D world only and
//! menus/HUD stay crisp – and only in the window's context.
//!
//! How: resolve the (multisampled) back buffer into our own texture with
//! glBlitFramebuffer, draw it back through our shader as a full-screen quad,
//! and restore every bit of GL state we touched. Any failure disables the
//! pipeline for the rest of the session instead of breaking the game.

pub mod gl;
pub mod shaders;
pub mod track;

use em4r_core::{info, warn};
use gl::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Effect parameters (from em4revive.toml).
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub sharpen: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub warmth: f32,
    pub filmic: f32,
    pub ssao: bool,
    pub ssao_strength: f32,
    pub ssao_radius: f32,
    pub dof: bool,
    pub dof_strength: f32,
    pub dof_start: f32,
    pub dof_end: f32,
}

static PARAMS: OnceLock<Params> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
/// Cleared if the depth buffer cannot be copied (then no SSAO/DOF).
static DEPTH_OK: AtomicBool = AtomicBool::new(true);

// Per-frame state (render thread).
static MAIN_DC: AtomicUsize = AtomicUsize::new(0);
static CUR_DC: AtomicUsize = AtomicUsize::new(0);
static SCENE_SEEN: AtomicBool = AtomicBool::new(false);
static PERSPECTIVE: AtomicBool = AtomicBool::new(false);
static APPLIED: AtomicBool = AtomicBool::new(false);
static RUNS: AtomicU32 = AtomicU32::new(0);
/// One-shot before/after screenshot for verification (`[fx] capture`).
static CAPTURE_DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

pub fn configure(enabled: bool, p: Params) {
    let _ = PARAMS.set(p);
    ENABLED.store(enabled, Ordering::Release);
}

/// Save one frame before/after the effects (once the game has been running
/// for 20 s) as BMP files into `dir`.
pub fn arm_capture(dir: std::path::PathBuf) {
    if let Ok(mut c) = CAPTURE_DIR.lock() {
        *c = Some(dir);
    }
}

/// Write a 32-bit RGBA bottom-up buffer (as glReadPixels returns it) as BMP.
pub fn write_bmp(path: &std::path::Path, w: i32, h: i32, rgba: &[u8]) -> std::io::Result<()> {
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes()); // positive = bottom-up, like GL
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in 0..h {
        let start = out.len();
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i]]);
        }
        while (out.len() - start) < row as usize {
            out.push(0);
        }
    }
    std::fs::write(path, out)
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && !FAILED.load(Ordering::Relaxed)
}

/// Buffer swap of the game window: remember its DC and start a new frame.
pub fn on_swap(hdc: usize) {
    MAIN_DC.store(hdc, Ordering::Relaxed);
    SCENE_SEEN.store(false, Ordering::Relaxed);
    APPLIED.store(false, Ordering::Relaxed);
}

fn in_main_context() -> bool {
    let cur = CUR_DC.load(Ordering::Relaxed);
    cur == 0 || cur == MAIN_DC.load(Ordering::Relaxed)
}

/// Projection parameters of the world camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub near: f32,
    pub far: f32,
    /// tan(half horizontal / vertical field of view)
    pub tan_x: f32,
    pub tan_y: f32,
}

static CAMERA: Mutex<Camera> = Mutex::new(Camera { near: 100.0, far: 32000.0, tan_x: 0.7, tan_y: 0.4 });

/// Called by the tracking hooks.
pub(crate) fn on_perspective(cam: Camera) {
    PERSPECTIVE.store(true, Ordering::Relaxed);
    // The world camera (near 100) – not the shadow camera (near 6000).
    if cam.near < 1000.0 && in_main_context() {
        SCENE_SEEN.store(true, Ordering::Relaxed);
        if let Ok(mut c) = CAMERA.try_lock() {
            *c = cam;
        }
    }
}

pub(crate) fn on_projection_2d() {
    PERSPECTIVE.store(false, Ordering::Relaxed);
}

pub(crate) fn on_make_current(dc: usize) {
    CUR_DC.store(dc, Ordering::Relaxed);
}

/// Called before every game draw call. Cheap unless it is time to run.
#[inline]
pub fn before_draw() {
    if !ENABLED.load(Ordering::Relaxed)
        || APPLIED.load(Ordering::Relaxed)
        || !SCENE_SEEN.load(Ordering::Relaxed)
        || PERSPECTIVE.load(Ordering::Relaxed)
        || FAILED.load(Ordering::Relaxed)
        || !in_main_context()
    {
        return;
    }
    APPLIED.store(true, Ordering::Relaxed);
    // SAFETY: we are on the render thread inside the game's GL context.
    unsafe { run() };
}

fn fail(reason: String) {
    if !FAILED.swap(true, Ordering::AcqRel) {
        warn!("Grafik-Effekte deaktiviert: {reason}");
    }
}

static GLFNS: OnceLock<Result<Gl, &'static str>> = OnceLock::new();
static PIPE: Mutex<Option<Pipeline>> = Mutex::new(None);

struct Pipeline {
    w: i32,
    h: i32,
    /// Scene copy: colour + depth/stencil textures.
    fbo: u32,
    color: u32,
    depth: u32,
    /// SSAO target.
    ao_fbo: u32,
    ao: u32,
    final_prog: u32,
    ao_prog: u32,
}

unsafe fn uniform(g: &Gl, prog: u32, name: &str) -> i32 {
    let c = format!("{name}\0");
    (g.glGetUniformLocation)(prog, c.as_ptr())
}

unsafe fn compile(g: &Gl, kind: u32, src: &str) -> Result<u32, String> {
    let s = (g.glCreateShader)(kind);
    let ptr = src.as_ptr();
    let len = src.len() as i32;
    (g.glShaderSource)(s, 1, &ptr, &len);
    (g.glCompileShader)(s);
    let mut ok = 0;
    (g.glGetShaderiv)(s, GL_COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = vec![0u8; 2048];
        let mut n = 0;
        (g.glGetShaderInfoLog)(s, buf.len() as i32, &mut n, buf.as_mut_ptr());
        return Err(format!("Shader-Fehler: {}", String::from_utf8_lossy(&buf[..n.max(0) as usize])));
    }
    Ok(s)
}

unsafe fn link(g: &Gl, vs: &str, fs: &str) -> Result<u32, String> {
    let p = (g.glCreateProgram)();
    (g.glAttachShader)(p, compile(g, GL_VERTEX_SHADER, vs)?);
    (g.glAttachShader)(p, compile(g, GL_FRAGMENT_SHADER, fs)?);
    (g.glLinkProgram)(p);
    let mut ok = 0;
    (g.glGetProgramiv)(p, GL_LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = vec![0u8; 2048];
        let mut n = 0;
        (g.glGetProgramInfoLog)(p, buf.len() as i32, &mut n, buf.as_mut_ptr());
        return Err(format!("Link-Fehler: {}", String::from_utf8_lossy(&buf[..n.max(0) as usize])));
    }
    Ok(p)
}

unsafe fn texture(g: &Gl, w: i32, h: i32) -> u32 {
    let mut t = 0;
    (g.glGenTextures)(1, &mut t);
    (g.glBindTexture)(GL_TEXTURE_2D, t);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    (g.glTexImage2D)(GL_TEXTURE_2D, 0, GL_RGBA8 as i32, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, core::ptr::null());
    t
}

/// Depth/stencil texture matching the window's D24S8 depth buffer
/// (glBlitFramebuffer needs identical depth formats).
unsafe fn depth_texture(g: &Gl, w: i32, h: i32) -> u32 {
    let mut t = 0;
    (g.glGenTextures)(1, &mut t);
    (g.glBindTexture)(GL_TEXTURE_2D, t);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_COMPARE_MODE, GL_NONE);
    (g.glTexImage2D)(
        GL_TEXTURE_2D, 0, GL_DEPTH24_STENCIL8 as i32, w, h, 0, GL_DEPTH_STENCIL, GL_UNSIGNED_INT_24_8, core::ptr::null(),
    );
    t
}

unsafe fn framebuffer(g: &Gl, color: u32, depth: Option<u32>) -> Result<u32, String> {
    let mut fbo = 0;
    (g.glGenFramebuffers)(1, &mut fbo);
    (g.glBindFramebuffer)(GL_FRAMEBUFFER, fbo);
    (g.glFramebufferTexture2D)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, color, 0);
    if let Some(d) = depth {
        (g.glFramebufferTexture2D)(GL_FRAMEBUFFER, GL_DEPTH_STENCIL_ATTACHMENT, GL_TEXTURE_2D, d, 0);
    }
    let status = (g.glCheckFramebufferStatus)(GL_FRAMEBUFFER);
    (g.glBindFramebuffer)(GL_FRAMEBUFFER, 0);
    if status != GL_FRAMEBUFFER_COMPLETE {
        return Err(format!("Framebuffer unvollständig (0x{status:X})"));
    }
    Ok(fbo)
}

impl Pipeline {
    unsafe fn create(g: &Gl, w: i32, h: i32) -> Result<Pipeline, String> {
        let final_prog = link(g, shaders::VERTEX, &shaders::final_pass())?;
        let ao_prog = link(g, shaders::VERTEX, &shaders::ssao())?;
        let color = texture(g, w, h);
        let depth = depth_texture(g, w, h);
        let fbo = framebuffer(g, color, Some(depth))?;
        let ao = texture(g, w, h);
        let ao_fbo = framebuffer(g, ao, None)?;
        info!("Grafik-Effekte bereit ({w}x{h})");
        Ok(Pipeline { w, h, fbo, color, depth, ao_fbo, ao, final_prog, ao_prog })
    }
}

unsafe fn fullscreen_quad(g: &Gl) {
    (g.glBegin)(GL_QUADS);
    (g.glVertex2f)(-1.0, -1.0);
    (g.glVertex2f)(1.0, -1.0);
    (g.glVertex2f)(1.0, 1.0);
    (g.glVertex2f)(-1.0, 1.0);
    (g.glEnd)();
}

#[cfg(test)]
mod tests {
    use super::*;
    use em4r_core::ffi::*;
    use em4r_core::util::{loaded_proc, wide};

    /// Real driver: create a hidden window + context, load all functions,
    /// compile/link the shaders and build the FBO.
    #[test]
    fn pipeline_builds_on_real_driver() {
        unsafe {
            LoadLibraryW(wide("opengl32.dll").as_ptr());
            let class = wide("STATIC");
            let hwnd = CreateWindowExW(
                0, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 256, 256,
                core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut(),
            );
            let dc = GetDC(hwnd);
            let pfd = PIXELFORMATDESCRIPTOR {
                nSize: PFD_SIZE,
                nVersion: 1,
                dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
                cColorBits: 32,
                cDepthBits: 24,
                cStencilBits: 8,
                ..Default::default()
            };
            let fmt = ChoosePixelFormat(dc, &pfd);
            assert!(SetPixelFormat(dc, fmt, &pfd) != 0);
            type Create = unsafe extern "system" fn(HDC) -> HGLRC;
            type MakeCurrent = unsafe extern "system" fn(HDC, HGLRC) -> BOOL;
            let create: Create = core::mem::transmute(loaded_proc("opengl32.dll", "wglCreateContext").unwrap());
            let make: MakeCurrent = core::mem::transmute(loaded_proc("opengl32.dll", "wglMakeCurrent").unwrap());
            let ctx = create(dc);
            assert!(!ctx.is_null());
            assert!(make(dc, ctx) != 0);

            let g = Gl::load().expect("GL-Funktionen");
            let _ = (g.glGetError)();
            let pl = Pipeline::create(&g, 256, 256).expect("Pipeline");
            assert!(pl.final_prog != 0 && pl.ao_prog != 0 && pl.fbo != 0 && pl.ao_fbo != 0);
            for name in ["colorTex", "depthTex", "aoTex", "sharpen", "aoStrength", "dofRange", "nearFar"] {
                assert!(uniform(&g, pl.final_prog, name) >= 0, "Uniform {name} fehlt im Abschluss-Shader");
            }
            for name in ["depthTex", "radius", "focal", "tanHalf"] {
                assert!(uniform(&g, pl.ao_prog, name) >= 0, "Uniform {name} fehlt im SSAO-Shader");
            }
            assert_eq!((g.glGetError)(), GL_NO_ERROR);

            make(core::ptr::null_mut(), core::ptr::null_mut());
            ReleaseDC(hwnd, dc);
            DestroyWindow(hwnd);
        }
    }
}

unsafe fn run() {
    let g = match GLFNS.get_or_init(|| Gl::load()) {
        Ok(g) => g,
        Err(name) => return fail(format!("OpenGL-Funktion {name} fehlt")),
    };
    let Some(p) = PARAMS.get().copied() else { return };
    let Ok(mut guard) = PIPE.try_lock() else { return };

    let mut vp = [0i32; 4];
    (g.glGetIntegerv)(GL_VIEWPORT, vp.as_mut_ptr());
    let (w, h) = (vp[2], vp[3]);
    if w < 64 || h < 64 {
        return;
    }
    // Save what glPushAttrib does not cover.
    let (mut prev_prog, mut prev_read, mut prev_draw, mut prev_active) = (0, 0, 0, 0);
    (g.glGetIntegerv)(GL_CURRENT_PROGRAM, &mut prev_prog);
    (g.glGetIntegerv)(GL_READ_FRAMEBUFFER_BINDING, &mut prev_read);
    (g.glGetIntegerv)(GL_DRAW_FRAMEBUFFER_BINDING, &mut prev_draw);
    (g.glGetIntegerv)(GL_ACTIVE_TEXTURE, &mut prev_active);
    let _ = (g.glGetError)(); // clear errors left by the game

    (g.glPushAttrib)(GL_ALL_ATTRIB_BITS);

    if guard.as_ref().is_none_or(|pl| pl.w != w || pl.h != h) {
        match Pipeline::create(g, w, h) {
            Ok(pl) => *guard = Some(pl),
            Err(e) => {
                (g.glPopAttrib)();
                (g.glBindFramebuffer)(GL_READ_FRAMEBUFFER, prev_read as u32);
                (g.glBindFramebuffer)(GL_DRAW_FRAMEBUFFER, prev_draw as u32);
                return fail(e);
            }
        }
    }
    let pl = guard.as_ref().expect("pipeline");

    // 1) Resolve the back buffer (MSAA) into our textures: colour always,
    //    depth too when SSAO/DOF need it (and the formats are compatible).
    let want_depth = (p.ssao || p.dof) && DEPTH_OK.load(Ordering::Relaxed);
    (g.glBindFramebuffer)(GL_READ_FRAMEBUFFER, prev_read as u32);
    (g.glBindFramebuffer)(GL_DRAW_FRAMEBUFFER, pl.fbo);
    let blit = |bits: u32| {
        (g.glBlitFramebuffer)(vp[0], vp[1], vp[0] + w, vp[1] + h, 0, 0, w, h, bits, GL_NEAREST as u32)
    };
    if want_depth {
        blit(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
        let e = (g.glGetError)();
        if e != GL_NO_ERROR {
            DEPTH_OK.store(false, Ordering::Relaxed);
            warn!("Tiefenpuffer nicht kopierbar (0x{e:X}) – SSAO und Tiefenunschärfe aus, Farbe/Schärfe bleiben");
            blit(GL_COLOR_BUFFER_BIT);
        }
    } else {
        blit(GL_COLOR_BUFFER_BIT);
    }
    let depth_ok = want_depth && DEPTH_OK.load(Ordering::Relaxed);
    let cam = CAMERA.lock().map(|c| *c).unwrap_or(Camera { near: 100.0, far: 32000.0, tan_x: 0.7, tan_y: 0.4 });

    // Verification screenshot (once, after 20 s of uninterrupted play).
    let capture = if em4r_core::counters::render_streak_ms() > 20_000.0 {
        CAPTURE_DIR.lock().ok().and_then(|mut c| c.take())
    } else {
        None
    };
    if let Some(dir) = &capture {
        let mut px = vec![0u8; (w * h * 4) as usize];
        (g.glBindFramebuffer)(GL_READ_FRAMEBUFFER, pl.fbo);
        (g.glReadPixels)(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, px.as_mut_ptr());
        let _ = write_bmp(&dir.join("em4revive_fx_vorher.bmp"), w, h, &px);
        (g.glBindFramebuffer)(GL_READ_FRAMEBUFFER, prev_read as u32);
    }

    // Common state for our full-screen passes.
    for cap in [GL_DEPTH_TEST, GL_BLEND, GL_ALPHA_TEST, GL_CULL_FACE, GL_SCISSOR_TEST, GL_STENCIL_TEST, GL_FOG, GL_LIGHTING] {
        (g.glDisable)(cap);
    }
    (g.glDepthMask)(0);
    (g.glColorMask)(1, 1, 1, 1);
    let texel = (1.0 / w as f32, 1.0 / h as f32);
    // Focal length in pixels per world unit at distance 1 (vertical).
    let focal = 0.5 * h as f32 / cam.tan_y.max(1e-3);
    let set_depth_uniforms = |prog: u32| {
        (g.glUniform1i)(uniform(g, prog, "depthTex"), 1);
        (g.glUniform2f)(uniform(g, prog, "nearFar"), cam.near, cam.far);
        (g.glUniform2f)(uniform(g, prog, "tanHalf"), cam.tan_x, cam.tan_y);
        (g.glUniform2f)(uniform(g, prog, "texel"), texel.0, texel.1);
    };
    (g.glActiveTexture)(GL_TEXTURE0 + 1);
    (g.glBindTexture)(GL_TEXTURE_2D, pl.depth);

    // 2) SSAO into its own texture.
    let ssao = p.ssao && depth_ok;
    if ssao {
        (g.glBindFramebuffer)(GL_DRAW_FRAMEBUFFER, pl.ao_fbo);
        (g.glViewport)(0, 0, w, h);
        (g.glUseProgram)(pl.ao_prog);
        set_depth_uniforms(pl.ao_prog);
        (g.glUniform1f)(uniform(g, pl.ao_prog, "radius"), p.ssao_radius);
        (g.glUniform1f)(uniform(g, pl.ao_prog, "focal"), focal);
        fullscreen_quad(g);
    }

    // 3) Final pass back into the game's frame.
    (g.glBindFramebuffer)(GL_DRAW_FRAMEBUFFER, prev_draw as u32);
    (g.glViewport)(vp[0], vp[1], w, h);
    (g.glActiveTexture)(GL_TEXTURE0);
    (g.glBindTexture)(GL_TEXTURE_2D, pl.color);
    (g.glActiveTexture)(GL_TEXTURE0 + 2);
    (g.glBindTexture)(GL_TEXTURE_2D, pl.ao);
    let fp = pl.final_prog;
    (g.glUseProgram)(fp);
    set_depth_uniforms(fp);
    let u1 = |name: &str, v: f32| (g.glUniform1f)(uniform(g, fp, name), v);
    (g.glUniform1i)(uniform(g, fp, "colorTex"), 0);
    (g.glUniform1i)(uniform(g, fp, "aoTex"), 2);
    u1("useAO", if ssao { 1.0 } else { 0.0 });
    u1("aoStrength", p.ssao_strength);
    u1("useDOF", if p.dof && depth_ok { 1.0 } else { 0.0 });
    u1("dofStrength", p.dof_strength);
    (g.glUniform2f)(uniform(g, fp, "dofRange"), p.dof_start, p.dof_end.max(p.dof_start + 1.0));
    u1("sharpen", p.sharpen);
    u1("contrast", p.contrast);
    u1("saturation", p.saturation);
    u1("vibrance", p.vibrance);
    u1("warmth", p.warmth);
    u1("filmic", p.filmic);
    fullscreen_quad(g);

    if let Some(dir) = &capture {
        let mut px = vec![0u8; (w * h * 4) as usize];
        (g.glReadBuffer)(GL_BACK);
        (g.glReadPixels)(vp[0], vp[1], w, h, GL_RGBA, GL_UNSIGNED_BYTE, px.as_mut_ptr());
        let _ = write_bmp(&dir.join("em4revive_fx_nachher.bmp"), w, h, &px);
        info!("Effekt-Vergleichsbilder gespeichert (em4revive_fx_vorher/nachher.bmp)");
    }

    // 3) Restore.
    (g.glUseProgram)(prev_prog as u32);
    (g.glPopAttrib)();
    (g.glActiveTexture)(prev_active as u32);
    (g.glBindFramebuffer)(GL_READ_FRAMEBUFFER, prev_read as u32);
    (g.glBindFramebuffer)(GL_DRAW_FRAMEBUFFER, prev_draw as u32);

    let err = (g.glGetError)();
    let n = RUNS.fetch_add(1, Ordering::Relaxed);
    if err != GL_NO_ERROR {
        return fail(format!("OpenGL-Fehler 0x{err:X} im Effekt-Durchgang"));
    }
    if n == 0 {
        info!(
            "Grafik-Effekte aktiv (Schärfe {:.2}, Kontrast {:.2}, Sättigung {:.2}, SSAO {}, Tiefenunschärfe {}; Kamera near {:.0} far {:.0})",
            p.sharpen,
            p.contrast,
            p.saturation,
            if ssao { "an" } else { "aus" },
            if p.dof && depth_ok { "an" } else { "aus" },
            cam.near,
            cam.far
        );
    }
}
