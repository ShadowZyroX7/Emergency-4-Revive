//! Tracking hooks: projection changes and context switches, per module.
//! (Same functions as the frame trace – the two features are exclusive.)

use em4r_core::hook::IatHook;
use em4r_core::iat_hook;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

static MATRIX_MODE: AtomicU32 = AtomicU32::new(0x1700);
const GL_PROJECTION: u32 = 0x1701;

/// Is a column-major projection matrix a perspective one?
pub fn is_perspective(m: &[f32; 16]) -> bool {
    m[15] == 0.0 && m[11] != 0.0
}

/// Near/far and half-angle tangents from a (symmetric) perspective matrix.
pub fn camera_from_matrix(m: &[f32; 16]) -> Option<crate::Camera> {
    if m[0] == 0.0 || m[5] == 0.0 || m[10] == 1.0 || m[10] == -1.0 {
        return None;
    }
    let near = (m[14] / (m[10] - 1.0)).abs();
    let far = (m[14] / (m[10] + 1.0)).abs();
    (near > 0.0 && far > near).then(|| crate::Camera { near, far, tan_x: 1.0 / m[0], tan_y: 1.0 / m[5] })
}

macro_rules! tracked_module {
    ($modname:ident) => {
        pub mod $modname {
            use super::*;
            static O_FRUSTUM: AtomicUsize = AtomicUsize::new(0);
            static O_LOADMATRIX: AtomicUsize = AtomicUsize::new(0);
            static O_MATRIXMODE: AtomicUsize = AtomicUsize::new(0);
            static O_LOADIDENTITY: AtomicUsize = AtomicUsize::new(0);
            static O_MAKECURRENT: AtomicUsize = AtomicUsize::new(0);

            unsafe extern "system" fn frustum(l: f64, r: f64, b: f64, t: f64, n: f64, f: f64) {
                let o: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64) =
                    core::mem::transmute(O_FRUSTUM.load(Ordering::Relaxed));
                o(l, r, b, t, n, f);
                if MATRIX_MODE.load(Ordering::Relaxed) == GL_PROJECTION && n > 0.0 {
                    crate::on_perspective(crate::Camera {
                        near: n as f32,
                        far: f as f32,
                        tan_x: ((r - l) / (2.0 * n)) as f32,
                        tan_y: ((t - b) / (2.0 * n)) as f32,
                    });
                }
            }
            unsafe extern "system" fn load_matrix(m: *const f32) {
                let o: unsafe extern "system" fn(*const f32) = core::mem::transmute(O_LOADMATRIX.load(Ordering::Relaxed));
                o(m);
                if MATRIX_MODE.load(Ordering::Relaxed) == GL_PROJECTION && !m.is_null() {
                    let mm = &*(m as *const [f32; 16]);
                    if is_perspective(mm) {
                        if let Some(cam) = camera_from_matrix(mm) {
                            crate::on_perspective(cam);
                        }
                    } else {
                        crate::on_projection_2d();
                    }
                }
            }
            unsafe extern "system" fn matrix_mode(mode: u32) {
                MATRIX_MODE.store(mode, Ordering::Relaxed);
                let o: unsafe extern "system" fn(u32) = core::mem::transmute(O_MATRIXMODE.load(Ordering::Relaxed));
                o(mode)
            }
            unsafe extern "system" fn load_identity() {
                let o: unsafe extern "system" fn() = core::mem::transmute(O_LOADIDENTITY.load(Ordering::Relaxed));
                o();
                if MATRIX_MODE.load(Ordering::Relaxed) == GL_PROJECTION {
                    crate::on_projection_2d();
                }
            }
            unsafe extern "system" fn make_current(dc: *mut core::ffi::c_void, rc: *mut core::ffi::c_void) -> i32 {
                let o: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32 =
                    core::mem::transmute(O_MAKECURRENT.load(Ordering::Relaxed));
                let r = o(dc, rc);
                crate::on_make_current(dc as usize);
                r
            }

            pub fn hooks() -> Vec<IatHook> {
                vec![
                    iat_hook!("opengl32.dll", "glFrustum", frustum, O_FRUSTUM),
                    iat_hook!("opengl32.dll", "glLoadMatrixf", load_matrix, O_LOADMATRIX),
                    iat_hook!("opengl32.dll", "glMatrixMode", matrix_mode, O_MATRIXMODE),
                    iat_hook!("opengl32.dll", "glLoadIdentity", load_identity, O_LOADIDENTITY),
                    iat_hook!("opengl32.dll", "wglMakeCurrent", make_current, O_MAKECURRENT),
                ]
            }
        }
    };
}

tracked_module!(vision);
tracked_module!(em4);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_detection() {
        // glFrustum-like matrix: m[11] = -1, m[15] = 0.
        let mut p = [0.0f32; 16];
        p[0] = 1.0;
        p[5] = 1.0;
        p[10] = -1.0063;
        p[11] = -1.0;
        p[14] = -200.6;
        assert!(is_perspective(&p));
        let mut o = [0.0f32; 16];
        o[0] = 1.0;
        o[5] = 1.0;
        o[10] = 1.0;
        o[15] = 1.0;
        assert!(!is_perspective(&o));
        // Matrix of glFrustum(near 100, far 32000), 90° fov.
        let (n, f) = (100.0f32, 32000.0f32);
        let mut q = [0.0f32; 16];
        q[0] = 1.0;
        q[5] = 1.0;
        q[10] = -(f + n) / (f - n);
        q[11] = -1.0;
        q[14] = -2.0 * f * n / (f - n);
        let cam = camera_from_matrix(&q).unwrap();
        assert!((cam.near - 100.0).abs() < 0.5, "{}", cam.near);
        assert!((cam.far - 32000.0).abs() < 200.0, "{}", cam.far);
        assert!((cam.tan_x - 1.0).abs() < 1e-6);
        assert!(camera_from_matrix(&[0.0; 16]).is_none());
    }
}
