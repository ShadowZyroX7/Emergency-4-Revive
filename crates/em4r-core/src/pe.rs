//! Locating and patching entries of a loaded module's import address table.
//!
//! Every module that statically imports e.g. `gdi32!SwapBuffers` calls it
//! through a pointer slot in its IAT. Replacing that slot redirects only the
//! calls made *by that module*. That is exactly what we want: the game's
//! modules get our wrappers, while ReShade and Windows stay untouched.

use crate::ffi::*;
use core::ffi::c_void;
use core::mem::size_of;

const IMAGE_DOS_SIGNATURE: u16 = 0x5A4D; // "MZ"
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550; // "PE\0\0"
const PE32_MAGIC: u16 = 0x10B;
const PE32PLUS_MAGIC: u16 = 0x20B;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;

#[inline]
unsafe fn rd16(addr: usize) -> u16 {
    (addr as *const u16).read_unaligned()
}
#[inline]
unsafe fn rd32(addr: usize) -> u32 {
    (addr as *const u32).read_unaligned()
}
#[inline]
unsafe fn rdusize(addr: usize) -> usize {
    (addr as *const usize).read_unaligned()
}

/// Compare a NUL-terminated ASCII string in memory with `s`.
unsafe fn cstr_eq(addr: usize, s: &str, ignore_case: bool) -> bool {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let c = *((addr + i) as *const u8);
        let equal = if ignore_case { c.eq_ignore_ascii_case(&b) } else { c == b };
        if c == 0 || !equal {
            return false;
        }
    }
    *((addr + bytes.len()) as *const u8) == 0
}

/// Base address of the import directory, if the module has one.
unsafe fn import_directory(base: usize) -> Option<usize> {
    if base == 0 || rd16(base) != IMAGE_DOS_SIGNATURE {
        return None;
    }
    let nt = base + rd32(base + 0x3C) as usize;
    if rd32(nt) != IMAGE_NT_SIGNATURE {
        return None;
    }
    let opt = nt + 24;
    let data_dirs = match rd16(opt) {
        PE32_MAGIC if size_of::<usize>() == 4 => opt + 96,
        PE32PLUS_MAGIC if size_of::<usize>() == 8 => opt + 112,
        _ => return None,
    };
    // Data directory #1 = imports.
    let rva = rd32(data_dirs + 8) as usize;
    (rva != 0).then_some(base + rva)
}

/// Find the IAT slot through which `module` calls `dll!func`.
///
/// # Safety
/// `module` must be a module mapped in this process.
pub unsafe fn find_iat_slot(module: HMODULE, dll: &str, func: &str) -> Option<*mut usize> {
    let base = module as usize;
    let mut desc = import_directory(base)?;
    let ts = size_of::<usize>();
    let ordinal_flag = 1usize << (ts * 8 - 1);
    // Lazily resolved address of the export, for modules without name thunks.
    let mut target: Option<Option<usize>> = None;

    loop {
        let oft = rd32(desc) as usize;
        let name = rd32(desc + 12) as usize;
        let ft = rd32(desc + 16) as usize;
        if name == 0 && ft == 0 {
            return None;
        }
        if name != 0 && ft != 0 && cstr_eq(base + name, dll, true) {
            let mut i = 0usize;
            loop {
                let slot = base + ft + i * ts;
                let bound = rdusize(slot);
                if bound == 0 {
                    break;
                }
                let matched = if oft != 0 {
                    let thunk = rdusize(base + oft + i * ts);
                    thunk & ordinal_flag == 0 && cstr_eq(base + (thunk & !ordinal_flag) + 2, func, false)
                } else {
                    // No name table: compare the bound address instead.
                    let t = *target.get_or_insert_with(|| crate::util::loaded_proc(dll, func));
                    t == Some(bound)
                };
                if matched {
                    return Some(slot as *mut usize);
                }
                i += 1;
            }
        }
        desc += IMPORT_DESCRIPTOR_SIZE;
    }
}

/// Write `value` into an IAT slot. Returns the previous value.
///
/// # Safety
/// `slot` must come from [`find_iat_slot`].
pub unsafe fn write_slot(slot: *mut usize, value: usize) -> Option<usize> {
    let mut old_protect = 0u32;
    if VirtualProtect(slot as *mut c_void, size_of::<usize>(), PAGE_READWRITE, &mut old_protect) == 0 {
        return None;
    }
    let previous = slot.read_volatile();
    slot.write_volatile(value);
    let mut dummy = 0u32;
    VirtualProtect(slot as *mut c_void, size_of::<usize>(), old_protect, &mut dummy);
    Some(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::GetModuleHandleW;

    // Called through our own `extern` declaration, which goes through the IAT
    // of the test executable – perfect for an end-to-end patching test.
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
    }

    unsafe extern "system" fn fake_pid() -> u32 {
        4242
    }

    #[test]
    fn finds_and_patches_own_import() {
        unsafe {
            let exe = GetModuleHandleW(core::ptr::null());
            let real = GetCurrentProcessId();
            let slot = find_iat_slot(exe, "KERNEL32.dll", "GetCurrentProcessId").expect("slot");
            // Case-insensitive DLL name.
            assert_eq!(find_iat_slot(exe, "kernel32.DLL", "GetCurrentProcessId"), Some(slot));

            let old = write_slot(slot, fake_pid as *const () as usize).unwrap();
            assert_eq!(core::hint::black_box(GetCurrentProcessId)(), 4242);
            write_slot(slot, old).unwrap();
            assert_eq!(core::hint::black_box(GetCurrentProcessId)(), real);
        }
    }

    #[test]
    fn unknown_imports_are_not_found() {
        unsafe {
            let exe = GetModuleHandleW(core::ptr::null());
            assert!(find_iat_slot(exe, "kernel32.dll", "NoSuchFunction").is_none());
            assert!(find_iat_slot(exe, "nosuch.dll", "GetCurrentProcessId").is_none());
            assert!(find_iat_slot(core::ptr::null_mut(), "kernel32.dll", "x").is_none());
        }
    }

    #[test]
    fn cstr_compare() {
        let s = b"Hello\0";
        unsafe {
            assert!(cstr_eq(s.as_ptr() as usize, "Hello", false));
            assert!(cstr_eq(s.as_ptr() as usize, "hello", true));
            assert!(!cstr_eq(s.as_ptr() as usize, "hello", false));
            assert!(!cstr_eq(s.as_ptr() as usize, "Hell", false));
            assert!(!cstr_eq(s.as_ptr() as usize, "Hello!", false));
        }
    }
}
