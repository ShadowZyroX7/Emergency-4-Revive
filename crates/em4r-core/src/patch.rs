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

/// Bytes of a `call rel32` at `site` that targets `target`.
pub fn call_rel32(site: usize, target: usize) -> [u8; 5] {
    let rel = (target as i64 - (site as i64 + 5)) as i32;
    let r = rel.to_le_bytes();
    [0xE8, r[0], r[1], r[2], r[3]]
}

/// Redirect the `call` at `site` from `original_target` to `new_target`,
/// but only if the instruction is exactly `call original_target`.
///
/// # Safety
/// `site` must be mapped code that no other thread executes right now.
pub unsafe fn redirect_call(site: usize, original_target: usize, new_target: usize) -> PatchResult {
    let expected = call_rel32(site, original_target);
    let replacement = call_rel32(site, new_target);
    apply_at(site as *mut u8, &expected, &replacement)
}

/// Do the bytes at `addr` match `expected` (code signature check)?
///
/// # Safety
/// `addr` must point to at least `expected.len()` mapped bytes.
pub unsafe fn matches(addr: usize, expected: &[u8]) -> bool {
    core::slice::from_raw_parts(addr as *const u8, expected.len()) == expected
}

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
    fn call_encoding_matches_em4_main_loop() {
        // Em4.exe 0x477C10: E8 7B 37 2C 00 = call 0x73B390 (ProcessMessages)
        assert_eq!(call_rel32(0x477C10, 0x73B390), [0xE8, 0x7B, 0x37, 0x2C, 0x00]);
        // Backwards target (negative rel32).
        let b = call_rel32(0x1000, 0x800);
        assert_eq!(i32::from_le_bytes([b[1], b[2], b[3], b[4]]), 0x800 - 0x1005);
    }

    #[test]
    fn redirect_call_in_buffer() {
        let mut buf = vec![0u8; 16];
        let site = buf.as_mut_ptr() as usize + 4;
        let (orig, new) = (site + 0x1000, site + 0x2000);
        buf[4..9].copy_from_slice(&call_rel32(site, orig));
        assert_eq!(unsafe { redirect_call(site, orig, new) }, PatchResult::Applied);
        assert_eq!(&buf[4..9], &call_rel32(site, new));
        // A call to something else is left alone.
        assert!(matches!(unsafe { redirect_call(site, orig + 1, new) }, PatchResult::AlreadyApplied | PatchResult::Mismatch(_)));
        assert!(unsafe { matches(site, &call_rel32(site, new)) });
    }

    #[test]
    fn jump_target_is_game_start() {
        // E9 rel32: target = address + 5 + rel32
        let rel = i32::from_le_bytes(SKIP_INTRO.replacement[1..5].try_into().unwrap());
        assert_eq!(0x0040_0000 + SKIP_INTRO.rva as i64 + 5 + rel as i64, 0x0047_9363);
    }
}
