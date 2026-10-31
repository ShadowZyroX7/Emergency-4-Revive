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

const DIR_IMPORT: usize = 1;
const DIR_DELAY_IMPORT: usize = 13;
const DELAY_DESCRIPTOR_SIZE: usize = 32;
const DELAY_ATTR_RVA: u32 = 1;

/// Address of data directory `index`, if the module has it.
unsafe fn data_directory(base: usize, index: usize) -> Option<usize> {
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
    let rva = rd32(data_dirs + index * 8) as usize;
    (rva != 0).then_some(base + rva)
}

unsafe fn import_directory(base: usize) -> Option<usize> {
    data_directory(base, DIR_IMPORT)
}

/// Index of `func` in a name table (INT) at `int_addr`, if present.
unsafe fn find_in_name_table(base: usize, int_addr: usize, func: &str) -> Option<usize> {
    let ts = size_of::<usize>();
    let ordinal_flag = 1usize << (ts * 8 - 1);
    let mut i = 0usize;
    loop {
        let thunk = rdusize(int_addr + i * ts);
        if thunk == 0 {
            return None;
        }
        if thunk & ordinal_flag == 0 && cstr_eq(base + (thunk & !ordinal_flag) + 2, func, false) {
            return Some(i);
        }
        i += 1;
    }
}

/// A slot in the *delay-load* IAT, plus the DLL name exactly as the module
/// names it (needed to resolve the real function the way the delay-load
/// helper would).
pub struct DelaySlot {
    pub slot: *mut usize,
    pub dll_name: String,
}

/// Find the delay-load IAT slot through which `module` calls `dll!func`.
///
/// Until first use, such a slot points to a stub that resolves the import
/// and then *overwrites the slot*. A hook must therefore never treat the
/// slot's current value as the original function – resolve it instead.
///
/// # Safety
/// `module` must be a module mapped in this process.
pub unsafe fn find_delay_iat_slot(module: HMODULE, dll: &str, func: &str) -> Option<DelaySlot> {
    let base = module as usize;
    let mut desc = data_directory(base, DIR_DELAY_IMPORT)?;
    loop {
        let attrs = rd32(desc);
        let name = rd32(desc + 4) as usize;
        let iat = rd32(desc + 12) as usize;
        let int = rd32(desc + 16) as usize;
        if name == 0 {
            return None;
        }
        // Old VC6-style descriptors store VAs instead of RVAs; not supported.
        if attrs & DELAY_ATTR_RVA != 0 && iat != 0 && int != 0 && cstr_eq(base + name, dll, true) {
            if let Some(i) = find_in_name_table(base, base + int, func) {
                let mut dll_name = String::new();
                let mut p = base + name;
                while *(p as *const u8) != 0 && dll_name.len() < 260 {
                    dll_name.push(*(p as *const u8) as char);
                    p += 1;
                }
                return Some(DelaySlot { slot: (base + iat + i * size_of::<usize>()) as *mut usize, dll_name });
            }
        }
        desc += DELAY_DESCRIPTOR_SIZE;
    }
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

    /// Uses the real vBase71.dll of the installed game (skipped if absent).
    /// Mapped with DONT_RESOLVE_DLL_REFERENCES: no imports, no DllMain.
    #[test]
    fn finds_delay_imports_of_real_vbase71() {
        let path = r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe\vBase71.dll";
        if !std::path::Path::new(path).exists() {
            eprintln!("vBase71.dll nicht vorhanden – Test übersprungen");
            return;
        }
        unsafe {
            let m = crate::ffi::LoadLibraryExW(crate::util::wide(path).as_ptr(), core::ptr::null_mut(), 0x1);
            assert!(!m.is_null());
            // These live only in the delay-load table.
            assert!(find_iat_slot(m, "gdi32.dll", "SwapBuffers").is_none());
            for f in ["SwapBuffers", "ChoosePixelFormat", "SetPixelFormat"] {
                let d = find_delay_iat_slot(m, "gdi32.dll", f).unwrap_or_else(|| panic!("{f} fehlt"));
                assert!(d.dll_name.eq_ignore_ascii_case("gdi32.dll"));
                assert!(!d.slot.is_null());
            }
            assert!(find_delay_iat_slot(m, "opengl32.dll", "wglGetProcAddress").is_some());
            assert!(find_delay_iat_slot(m, "gdi32.dll", "NoSuchFunction").is_none());
            crate::ffi::FreeLibrary(m);
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
