//! Texture filtering: anisotropic filtering and trilinear mipmapping.
//!
//! The engine sets filters with `glTexParameteri/f` when it creates
//! textures. We upgrade bilinear mipmapping to trilinear and attach the
//! configured anisotropy level whenever a mipmapped min filter is set.

use em4r_core::hook::IatHook;
use em4r_core::{iat_hook, info};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
pub const GL_NEAREST_MIPMAP_NEAREST: i32 = 0x2700;
pub const GL_LINEAR_MIPMAP_NEAREST: i32 = 0x2701;
pub const GL_NEAREST_MIPMAP_LINEAR: i32 = 0x2702;
pub const GL_LINEAR_MIPMAP_LINEAR: i32 = 0x2703;
pub const GL_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
pub const GL_MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;

type TexParamI = unsafe extern "system" fn(u32, u32, i32);
type TexParamF = unsafe extern "system" fn(u32, u32, f32);
type GetFloatV = unsafe extern "system" fn(u32, *mut f32);

static ORIG_TEXPARAM_I: AtomicUsize = AtomicUsize::new(0);
static ORIG_TEXPARAM_F: AtomicUsize = AtomicUsize::new(0);

/// Effective anisotropy as f32 bits. 0 = not yet determined,
/// `u32::MAX` = unsupported/disabled.
static EFFECTIVE_AF: AtomicU32 = AtomicU32::new(0);
const AF_DISABLED: u32 = u32::MAX;

pub fn hooks() -> Vec<IatHook> {
    vec![
        iat_hook!("opengl32.dll", "glTexParameteri", hk_tex_parameteri, ORIG_TEXPARAM_I),
        iat_hook!("opengl32.dll", "glTexParameterf", hk_tex_parameterf, ORIG_TEXPARAM_F),
    ]
}

pub fn is_mipmapped(filter: i32) -> bool {
    (GL_NEAREST_MIPMAP_NEAREST..=GL_LINEAR_MIPMAP_LINEAR).contains(&filter)
}

/// Upgrade the min filter if trilinear is forced.
pub fn upgrade_min_filter(filter: i32, force_trilinear: bool) -> i32 {
    if force_trilinear && filter == GL_LINEAR_MIPMAP_NEAREST {
        GL_LINEAR_MIPMAP_LINEAR
    } else {
        filter
    }
}

/// Clamp the wanted level to what the driver supports.
pub fn effective_level(wanted: u32, driver_max: f32) -> Option<f32> {
    if wanted == 0 || !(driver_max >= 1.0) {
        return None;
    }
    Some((wanted as f32).min(driver_max).max(1.0))
}

fn gl_proc(name: &str) -> Option<usize> {
    em4r_core::util::loaded_proc("opengl32.dll", name)
}

/// The anisotropy level to apply, determined once a context exists.
fn anisotropy() -> Option<f32> {
    let bits = EFFECTIVE_AF.load(Ordering::Relaxed);
    if bits == AF_DISABLED {
        return None;
    }
    if bits != 0 {
        return Some(f32::from_bits(bits));
    }
    let wanted = crate::settings().anisotropy;
    let mut max = 0.0f32;
    if wanted > 0 {
        if let Some(p) = gl_proc("glGetFloatv") {
            // SAFETY: glGetFloatv signature; a context is current because the
            // engine is setting texture parameters right now.
            unsafe { core::mem::transmute::<usize, GetFloatV>(p)(GL_MAX_TEXTURE_MAX_ANISOTROPY, &mut max) };
        }
    }
    let level = effective_level(wanted, max);
    match level {
        Some(l) => {
            info!("Anisotrope Filterung: {l}x (Treiber-Maximum {max}x)");
            EFFECTIVE_AF.store(l.to_bits(), Ordering::Relaxed);
        }
        None => {
            if wanted > 0 {
                info!("Anisotrope Filterung nicht verfügbar (Treiber meldet {max})");
            }
            EFFECTIVE_AF.store(AF_DISABLED, Ordering::Relaxed);
        }
    }
    level
}

unsafe fn set_af(target: u32) {
    let Some(level) = anisotropy() else { return };
    let orig = ORIG_TEXPARAM_F.load(Ordering::Relaxed);
    let addr = if orig != 0 { Some(orig) } else { gl_proc("glTexParameterf") };
    if let Some(a) = addr {
        core::mem::transmute::<usize, TexParamF>(a)(target, GL_TEXTURE_MAX_ANISOTROPY, level);
    }
}

unsafe extern "system" fn hk_tex_parameteri(target: u32, pname: u32, param: i32) {
    let f: TexParamI = core::mem::transmute(ORIG_TEXPARAM_I.load(Ordering::Relaxed));
    let s = crate::settings();
    match pname {
        GL_TEXTURE_MIN_FILTER => {
            let filter = upgrade_min_filter(param, s.force_trilinear);
            f(target, pname, filter);
            if s.anisotropy > 0 && is_mipmapped(filter) {
                set_af(target);
            }
        }
        // The game sets its own (low) anisotropy: replace it with ours.
        GL_TEXTURE_MAX_ANISOTROPY if s.anisotropy > 0 => set_af(target),
        _ => f(target, pname, param),
    }
}

unsafe extern "system" fn hk_tex_parameterf(target: u32, pname: u32, param: f32) {
    let f: TexParamF = core::mem::transmute(ORIG_TEXPARAM_F.load(Ordering::Relaxed));
    let s = crate::settings();
    match pname {
        GL_TEXTURE_MIN_FILTER => {
            let filter = upgrade_min_filter(param as i32, s.force_trilinear);
            f(target, pname, filter as f32);
            if s.anisotropy > 0 && is_mipmapped(filter) {
                set_af(target);
            }
        }
        GL_TEXTURE_MAX_ANISOTROPY if s.anisotropy > 0 => set_af(target),
        _ => f(target, pname, param),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mipmap_detection() {
        assert!(is_mipmapped(GL_LINEAR_MIPMAP_LINEAR));
        assert!(is_mipmapped(GL_NEAREST_MIPMAP_NEAREST));
        assert!(!is_mipmapped(0x2601)); // GL_LINEAR
        assert!(!is_mipmapped(0x2600)); // GL_NEAREST
    }

    #[test]
    fn trilinear_upgrade() {
        assert_eq!(upgrade_min_filter(GL_LINEAR_MIPMAP_NEAREST, true), GL_LINEAR_MIPMAP_LINEAR);
        assert_eq!(upgrade_min_filter(GL_LINEAR_MIPMAP_NEAREST, false), GL_LINEAR_MIPMAP_NEAREST);
        assert_eq!(upgrade_min_filter(GL_NEAREST_MIPMAP_NEAREST, true), GL_NEAREST_MIPMAP_NEAREST);
        assert_eq!(upgrade_min_filter(0x2601, true), 0x2601);
    }

    #[test]
    fn anisotropy_clamping() {
        assert_eq!(effective_level(16, 16.0), Some(16.0));
        assert_eq!(effective_level(16, 8.0), Some(8.0));
        assert_eq!(effective_level(4, 16.0), Some(4.0));
        assert_eq!(effective_level(0, 16.0), None);
        assert_eq!(effective_level(16, 0.0), None);
        assert_eq!(effective_level(16, f32::NAN), None);
    }
}
