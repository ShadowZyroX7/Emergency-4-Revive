//! Small helpers: wide strings, module paths, timing, memory info.

use crate::ffi::*;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// NUL-terminated UTF-16 string for Win32 `W` functions.
pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Full path of a loaded module (`None` = the main executable).
pub fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: buffer pointer and length are valid.
        let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(PathBuf::from(OsString::from_wide(&buf[..n])));
        }
        if buf.len() >= 32 * 1024 {
            return None;
        }
        buf.resize(buf.len() * 2, 0);
    }
}

/// Lower-case file name of a path, e.g. `vision71.dll`.
pub fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Resolve an export from an already-loaded module without loading it.
pub fn loaded_proc(dll: &str, func: &str) -> Option<usize> {
    let wdll = wide(dll);
    let cfunc: Vec<u8> = func.bytes().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated.
    unsafe {
        let m = GetModuleHandleW(wdll.as_ptr());
        if m.is_null() {
            return None;
        }
        let p = GetProcAddress(m, cfunc.as_ptr());
        (!p.is_null()).then_some(p as usize)
    }
}

pub fn qpc_freq() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: valid out pointer.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    })
}

pub fn qpc_now() -> i64 {
    let mut v = 0i64;
    // SAFETY: valid out pointer.
    unsafe { QueryPerformanceCounter(&mut v) };
    v
}

pub fn qpc_to_ms(ticks: i64) -> f64 {
    ticks as f64 * 1000.0 / qpc_freq() as f64
}

/// (private bytes, working set) of this process in bytes.
pub fn process_memory() -> Option<(usize, usize)> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: core::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: valid struct with correct cb.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some((c.PrivateUsage, c.WorkingSetSize))
}


/// (used, total) user address space of this process in bytes. For a 32-bit
/// Large-Address-Aware process on 64-bit Windows the total is 4 GB.
pub fn address_space() -> Option<(u64, u64)> {
    let mut s = MEMORYSTATUSEX { dwLength: core::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: valid struct with correct length.
    let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
    (ok != 0 && s.ullTotalVirtual > 0).then(|| (s.ullTotalVirtual - s.ullAvailVirtual, s.ullTotalVirtual))
}
pub fn local_time() -> SYSTEMTIME {
    let mut st = SYSTEMTIME::default();
    // SAFETY: valid out pointer.
    unsafe { GetLocalTime(&mut st) };
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide("ab"), vec![b'a' as u16, b'b' as u16, 0]);
    }

    #[test]
    fn finds_own_exe_and_kernel32_export() {
        let exe = module_path(core::ptr::null_mut()).unwrap();
        assert!(exe.exists());
        assert!(loaded_proc("kernel32.dll", "GetProcAddress").is_some());
        assert!(loaded_proc("kernel32.dll", "NoSuchFunction123").is_none());
        assert!(loaded_proc("not_loaded_module.dll", "x").is_none());
    }

    #[test]
    fn memory_info_works() {
        let (private, ws) = process_memory().unwrap();
        assert!(private > 0 && ws > 0);
    }
}
