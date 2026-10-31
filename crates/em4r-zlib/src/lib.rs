//! Faster decompression for EM4: vBase71.dll embeds zlib 1.2.1 (2003); its
//! inflate is ~11 % of the loading time (models, PNG textures, ZIP files).
//!
//! The five inflate entry points used by the DLL's own code (ZIP file
//! system and libpng) are redirected – a `jmp` at each function start – to
//! zlib-rs, a modern implementation with the same API and `z_stream`
//! layout. The whole family is replaced together, because a stream must be
//! initialised, inflated, reset and ended by the same implementation.
//! The old internal helpers (inflate_fast, updatewindow, inflate_table) are
//! only reachable from the old `inflate` and simply become unused.
//!
//! Function locations (vBase71.dll, image base 0x10000000), identified by
//! their zlib error strings and call graph:
//!
//! | RVA     | function      | used by         |
//! |---------|---------------|-----------------|
//! | 0x21AA0 | inflateReset  | libpng          |
//! | 0x21AF0 | inflateInit2_ | ZIP file system |
//! | 0x21BC0 | inflateInit_  | libpng          |
//! | 0x21D00 | inflate       | ZIP + libpng    |
//! | 0x23160 | inflateEnd    | ZIP + libpng    |
//!
//! All are cdecl. Every function start is verified byte-for-byte first (the
//! signatures contain no relocated addresses, the DLL is rebased at load).

use core::ffi::{c_char, c_int, c_void};
use em4r_core::patch::{self, PatchResult};
use em4r_core::{info, warn};
use libz_rs_sys::z_stream;
use std::sync::atomic::{AtomicU64, Ordering};

pub static INFLATE_CALLS: AtomicU64 = AtomicU64::new(0);

pub unsafe extern "C" fn rs_inflate_reset(strm: *mut c_void) -> c_int {
    libz_rs_sys::inflateReset(strm as *mut z_stream)
}

pub unsafe extern "C" fn rs_inflate_init2(strm: *mut c_void, window_bits: c_int, version: *const c_char, size: c_int) -> c_int {
    libz_rs_sys::inflateInit2_(strm as *mut z_stream, window_bits, version, size)
}

pub unsafe extern "C" fn rs_inflate_init(strm: *mut c_void, version: *const c_char, size: c_int) -> c_int {
    libz_rs_sys::inflateInit_(strm as *mut z_stream, version, size)
}

pub unsafe extern "C" fn rs_inflate(strm: *mut c_void, flush: c_int) -> c_int {
    INFLATE_CALLS.fetch_add(1, Ordering::Relaxed);
    libz_rs_sys::inflate(strm as *mut z_stream, flush)
}

pub unsafe extern "C" fn rs_inflate_end(strm: *mut c_void) -> c_int {
    libz_rs_sys::inflateEnd(strm as *mut z_stream)
}

struct Target {
    name: &'static str,
    rva: usize,
    signature: &'static [u8],
    replacement: usize,
}

fn targets() -> [Target; 5] {
    [
        Target {
            name: "inflateReset",
            rva: 0x21AA0,
            signature: &[0x8B, 0x54, 0x24, 0x04, 0x33, 0xC9, 0x3B, 0xD1, 0x74, 0x39],
            replacement: rs_inflate_reset as *const () as usize,
        },
        Target {
            name: "inflateInit2_",
            rva: 0x21AF0,
            signature: &[0x8B, 0x44, 0x24, 0x0C, 0x57, 0x33, 0xFF, 0x3B, 0xC7, 0x0F],
            replacement: rs_inflate_init2 as *const () as usize,
        },
        Target {
            name: "inflateInit_",
            rva: 0x21BC0,
            signature: &[0x8B, 0x44, 0x24, 0x0C, 0x8B, 0x4C, 0x24, 0x08, 0x8B, 0x54],
            replacement: rs_inflate_init as *const () as usize,
        },
        Target {
            name: "inflate",
            rva: 0x21D00,
            signature: &[0x8B, 0x44, 0x24, 0x04, 0x83, 0xEC, 0x38, 0x85, 0xC0, 0x53],
            replacement: rs_inflate as *const () as usize,
        },
        Target {
            name: "inflateEnd",
            rva: 0x23160,
            signature: &[0x56, 0x8B, 0x74, 0x24, 0x08, 0x85, 0xF6, 0x74, 0x38, 0x8B],
            replacement: rs_inflate_end as *const () as usize,
        },
    ]
}

/// `jmp rel32` from `site` to `target`.
pub fn jmp_rel32(site: usize, target: usize) -> [u8; 5] {
    let rel = (target as i64 - (site as i64 + 5)) as i32;
    let r = rel.to_le_bytes();
    [0xE9, r[0], r[1], r[2], r[3]]
}

/// Redirect vBase71's inflate family to zlib-rs. All-or-nothing: if any
/// signature differs, nothing is patched.
///
/// # Safety
/// Call during DllMain at process start (no game code running yet).
pub unsafe fn install() -> bool {
    let module = em4r_core::ffi::GetModuleHandleW(em4r_core::util::wide("vBase71.dll").as_ptr());
    if module.is_null() {
        warn!("Schnelles Entpacken: vBase71.dll nicht geladen – übersprungen");
        return false;
    }
    let base = module as usize;
    let ts = targets();
    for t in &ts {
        if !patch::matches(base + t.rva, t.signature) {
            warn!("Schnelles Entpacken: {} hat unbekannte Bytes (andere Spielversion) – übersprungen", t.name);
            return false;
        }
    }
    for t in &ts {
        let site = base + t.rva;
        let expected = &t.signature[..5];
        let r = patch::apply_at(site as *mut u8, expected, &jmp_rel32(site, t.replacement));
        if r != PatchResult::Applied {
            // Cannot happen after the check above; stop to avoid a mixed state.
            warn!("Schnelles Entpacken: {} nicht patchbar ({r:?})", t.name);
            return false;
        }
    }
    info!("Patch aktiv: Schnelles Entpacken (zlib 1.2.1 -> zlib-rs)");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use libz_rs_sys::*;

    fn sample() -> Vec<u8> {
        // Compressible but not trivial: a repeated text with a counter.
        (0..20_000).flat_map(|i| format!("EM4 Revive {i} Feuerwehr Auenburg\n").into_bytes()).collect()
    }

    unsafe fn deflate_with(window_bits: c_int, data: &[u8]) -> Vec<u8> {
        let mut s: z_stream = core::mem::zeroed();
        let ver = b"1.2.1\0".as_ptr() as *const c_char;
        assert_eq!(
            deflateInit2_(&mut s, 6, Z_DEFLATED, window_bits, 8, Z_DEFAULT_STRATEGY, ver, core::mem::size_of::<z_stream>() as c_int),
            Z_OK
        );
        let mut out = vec![0u8; data.len() + 1024];
        s.next_in = data.as_ptr() as *mut u8;
        s.avail_in = data.len() as u32;
        s.next_out = out.as_mut_ptr();
        s.avail_out = out.len() as u32;
        assert_eq!(deflate(&mut s, Z_FINISH), Z_STREAM_END);
        out.truncate(s.total_out as usize);
        deflateEnd(&mut s);
        out
    }

    /// Feed `input` in small pieces like the engine's readers do.
    unsafe fn inflate_chunked(s: &mut z_stream, input: &[u8], expect_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; expect_len];
        s.next_out = out.as_mut_ptr();
        s.avail_out = out.len() as u32;
        let mut pos = 0;
        loop {
            let n = (input.len() - pos).min(4096);
            s.next_in = input.as_ptr().add(pos) as *mut u8;
            s.avail_in = n as u32;
            let r = rs_inflate(s as *mut z_stream as *mut c_void, Z_NO_FLUSH);
            pos += n - s.avail_in as usize;
            if r == Z_STREAM_END {
                break;
            }
            assert_eq!(r, Z_OK, "inflate-Fehler");
        }
        out.truncate(s.total_out as usize);
        out
    }

    #[test]
    fn png_style_zlib_stream_with_reset() {
        unsafe {
            let data = sample();
            let comp = deflate_with(15, &data);
            let mut s: z_stream = core::mem::zeroed();
            let ver = b"1.2.1\0".as_ptr() as *const c_char;
            // libpng: inflateInit_ with the 1.2.1 version string and size 0x38.
            assert_eq!(core::mem::size_of::<z_stream>(), 0x38);
            assert_eq!(rs_inflate_init(&mut s as *mut z_stream as *mut c_void, ver, 0x38), Z_OK);
            assert_eq!(inflate_chunked(&mut s, &comp, data.len()), data);
            // libpng resets the same stream for the next image.
            assert_eq!(rs_inflate_reset(&mut s as *mut z_stream as *mut c_void), Z_OK);
            assert_eq!(inflate_chunked(&mut s, &comp, data.len()), data);
            assert_eq!(rs_inflate_end(&mut s as *mut z_stream as *mut c_void), Z_OK);
            assert!(s.state.is_null());
        }
    }

    #[test]
    fn zip_style_raw_deflate() {
        unsafe {
            let data = sample();
            let comp = deflate_with(-15, &data);
            let mut s: z_stream = core::mem::zeroed();
            let ver = b"1.2.1\0".as_ptr() as *const c_char;
            // VFSZipDir: inflateInit2_(strm, -15, "1.2.1", 0x38).
            assert_eq!(rs_inflate_init2(&mut s as *mut z_stream as *mut c_void, -15, ver, 0x38), Z_OK);
            assert_eq!(inflate_chunked(&mut s, &comp, data.len()), data);
            assert_eq!(rs_inflate_end(&mut s as *mut z_stream as *mut c_void), Z_OK);
        }
    }

    #[test]
    fn rejects_wrong_struct_size_like_zlib() {
        unsafe {
            let mut s: z_stream = core::mem::zeroed();
            let ver = b"1.2.1\0".as_ptr() as *const c_char;
            assert_eq!(rs_inflate_init(&mut s as *mut z_stream as *mut c_void, ver, 0x30), Z_VERSION_ERROR);
        }
    }

    #[test]
    fn jump_encoding() {
        assert_eq!(jmp_rel32(0x1000, 0x1010), [0xE9, 0x0B, 0, 0, 0]);
        let back = jmp_rel32(0x2000, 0x1000);
        assert_eq!(i32::from_le_bytes([back[1], back[2], back[3], back[4]]), 0x1000 - 0x2005);
    }

    /// The signatures must match the installed vBase71.dll (mapped as data
    /// image without running it).
    #[test]
    fn signatures_match_installed_dll() {
        let path = r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe\vBase71.dll";
        if !std::path::Path::new(path).exists() {
            return;
        }
        unsafe {
            // DONT_RESOLVE_DLL_REFERENCES: mapped like a normal image (incl.
            // relocations), but DllMain does not run.
            let m = em4r_core::ffi::LoadLibraryExW(em4r_core::util::wide(path).as_ptr(), core::ptr::null_mut(), 0x1);
            assert!(!m.is_null());
            for t in targets() {
                assert!(patch::matches(m as usize + t.rva, t.signature), "{} passt nicht", t.name);
            }
            em4r_core::ffi::FreeLibrary(m);
        }
    }
}
