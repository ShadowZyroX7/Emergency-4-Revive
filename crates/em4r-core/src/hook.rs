//! IAT hook descriptors and installation across the game's modules.

use crate::ffi::*;
use crate::pe;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One redirected import: calls to `dll!func` go to `detour`; the detour
/// calls the real function through `original`.
pub struct IatHook {
    pub dll: &'static str,
    pub func: &'static str,
    pub detour: usize,
    pub original: &'static AtomicUsize,
}

impl IatHook {
    /// The real function, or 0 if the hook was never installed.
    #[inline]
    pub fn original(&self) -> usize {
        self.original.load(Ordering::Acquire)
    }
}

/// Declare a hook: `iat_hook!("gdi32.dll", "SwapBuffers", hk_swap, ORIG_SWAP)`.
#[macro_export]
macro_rules! iat_hook {
    ($dll:expr, $func:expr, $detour:expr, $orig:expr) => {
        $crate::hook::IatHook { dll: $dll, func: $func, detour: $detour as *const () as usize, original: &$orig }
    };
}

/// Result of installing one hook into one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub module: String,
    pub func: &'static str,
}

/// Install `hooks` into `module`. Idempotent: slots that already point to
/// our detour are skipped.
///
/// # Safety
/// `module` must be a module mapped in this process.
pub unsafe fn install_into(module: HMODULE, module_name: &str, hooks: &[IatHook]) -> Vec<Installed> {
    let mut done = Vec::new();
    for h in hooks {
        let Some(slot) = pe::find_iat_slot(module, h.dll, h.func) else { continue };
        let current = slot.read_volatile();
        if current == h.detour {
            continue;
        }
        // Publish the original *before* redirecting, so a concurrent call
        // through the new slot always finds it.
        match h.original.compare_exchange(0, current, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {}
            Err(existing) if existing == current => {}
            Err(existing) => {
                // Another module is bound to a different implementation
                // (e.g. someone else hooked it). Chaining would be wrong
                // for one of the two, so leave this module alone.
                crate::warn!(
                    "{}: {}!{} zeigt auf {:#x}, erwartet {:#x} – übersprungen",
                    module_name, h.dll, h.func, current, existing
                );
                continue;
            }
        }
        if pe::write_slot(slot, h.detour).is_some() {
            done.push(Installed { module: module_name.to_string(), func: h.func });
        } else {
            crate::error!("{}: Konnte {}!{} nicht patchen", module_name, h.dll, h.func);
        }
    }
    done
}

/// All loaded modules whose file lives inside `game_dir`, except those in
/// `exclude` (lower-case file names) and `own`.
pub fn game_modules(game_dir: &Path, own: HMODULE, exclude: &[&str]) -> Vec<(HMODULE, String)> {
    let mut handles = vec![core::ptr::null_mut(); 1024];
    let mut needed = 0u32;
    // SAFETY: buffer and sizes are valid.
    let ok = unsafe {
        K32EnumProcessModules(
            GetCurrentProcess(),
            handles.as_mut_ptr(),
            (handles.len() * core::mem::size_of::<HMODULE>()) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let count = (needed as usize / core::mem::size_of::<HMODULE>()).min(handles.len());
    let dir = game_dir.to_string_lossy().to_ascii_lowercase();
    handles[..count]
        .iter()
        .filter(|&&m| m != own)
        .filter_map(|&m| {
            let path = crate::util::module_path(m)?;
            let in_dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_ascii_lowercase() == dir)
                .unwrap_or(false);
            let name = crate::util::file_name_lower(&path);
            (in_dir && !exclude.contains(&name.as_str())).then_some((m, name))
        })
        .collect()
}
