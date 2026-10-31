//! Verified in-memory code patches for Em4.exe.
//!
//! A patch only applies if the bytes at its address are *exactly* the
//! expected original bytes. Other game versions are therefore left alone
//! (logged as a mismatch) instead of being corrupted. Nothing is written to
//! disk: the patch lives only in the running process.

use crate::ffi::*;
use core::ffi::c_void;

pub struct BytePatch {
    pub name: &'static str,
    /// Offset from the module base.
    pub rva: usize,
    pub expected: &'static [u8],
    pub replacement: &'static [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchResult {
    Applied,
    AlreadyApplied,
    /// Unexpected bytes found (different game version) – nothing changed.
    Mismatch(Vec<u8>),
    Failed,
}

/// Skip the Sixteen Tons logo, the 1 s pause and the intro video.
///
/// Em4.exe (Deluxe, Steam), function at 0x479300:
/// ```text
/// 0047932E  push "base:/Video/16t.mpg"   ; 68 20 68 9D 00
/// 00479333  call PlayVideo
/// 0047933B  push 3E8h / call Sleep        ; 1000 ms
/// 00479346  push "base:/Video/Intro.mpg"
/// 0047934B  call PlayVideo
/// ...
/// 00479363  (game start continues)
/// ```
/// The first instruction becomes `jmp 00479363` (E9 30 00 00 00).
pub const SKIP_INTRO: BytePatch = BytePatch {
    name: "Intro überspringen",
    rva: 0x7932E,
    expected: &[0x68, 0x20, 0x68, 0x9D, 0x00],
    replacement: &[0xE9, 0x30, 0x00, 0x00, 0x00],
};

/// Apply `p` to the module at `base`.
///
/// # Safety
/// `base` must be the base of a mapped module containing `p.rva`, and no
/// other thread may execute the patched bytes at this moment (true during
/// DllMain at process start).
pub unsafe fn apply(base: HMODULE, p: &BytePatch) -> PatchResult {
    apply_at((base as usize + p.rva) as *mut u8, p.expected, p.replacement)
}

/// # Safety
/// `addr` must point to at least `expected.len()` mapped bytes.
pub unsafe fn apply_at(addr: *mut u8, expected: &[u8], replacement: &[u8]) -> PatchResult {
    if expected.len() != replacement.len() || expected.is_empty() {
        return PatchResult::Failed;
    }
    let len = expected.len();
    let current = core::slice::from_raw_parts(addr, len);
    if current == replacement {
        return PatchResult::AlreadyApplied;
    }
    if current != expected {
        return PatchResult::Mismatch(current.to_vec());
    }
    let mut old = 0u32;
    if VirtualProtect(addr as *mut c_void, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return PatchResult::Failed;
    }
    core::ptr::copy_nonoverlapping(replacement.as_ptr(), addr, len);
    let mut dummy = 0u32;
    VirtualProtect(addr as *mut c_void, len, old, &mut dummy);
    FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, len);
    PatchResult::Applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_over_expected_bytes() {
        let mut buf = vec![0x90u8, 0x68, 0x20, 0x68, 0x9D, 0x00, 0x90];
        let p = unsafe { buf.as_mut_ptr().add(1) };
        let (exp, rep) = (SKIP_INTRO.expected, SKIP_INTRO.replacement);
        assert_eq!(unsafe { apply_at(p, exp, rep) }, PatchResult::Applied);
        assert_eq!(buf, [0x90, 0xE9, 0x30, 0x00, 0x00, 0x00, 0x90]);
        // Idempotent.
        assert_eq!(unsafe { apply_at(buf.as_mut_ptr().add(1), exp, rep) }, PatchResult::AlreadyApplied);
    }

    #[test]
    fn leaves_unknown_versions_alone() {
        let mut buf = vec![0x55u8, 0x8B, 0xEC, 0x83, 0xEC];
        let before = buf.clone();
        let r = unsafe { apply_at(buf.as_mut_ptr(), SKIP_INTRO.expected, SKIP_INTRO.replacement) };
        assert_eq!(r, PatchResult::Mismatch(before.clone()));
        assert_eq!(buf, before);
    }

    #[test]
    fn jump_target_is_game_start() {
        // E9 rel32: target = address + 5 + rel32
        let rel = i32::from_le_bytes(SKIP_INTRO.replacement[1..5].try_into().unwrap());
        assert_eq!(0x0040_0000 + SKIP_INTRO.rva as i64 + 5 + rel as i64, 0x0047_9363);
    }
}
