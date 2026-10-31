//! Minimal hand-written Win32 bindings.
//!
//! Only what the mod needs is declared here. Declaring these by hand keeps
//! the build independent of the API churn in the big binding crates.
//! All functions use the `system` ABI (stdcall on x86).
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type BOOL = i32;
pub type HANDLE = *mut c_void;
pub type HMODULE = *mut c_void;
pub type HDC = *mut c_void;
pub type HGLRC = *mut c_void;
pub type FARPROC = *mut c_void;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const DLL_PROCESS_DETACH: u32 = 0;

pub const PAGE_READWRITE: u32 = 0x04;
pub const INFINITE: u32 = 0xFFFF_FFFF;
pub const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002;
pub const TIMER_ALL_ACCESS: u32 = 0x001F_0003;
pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

// Pixel format flags.
pub const PFD_DOUBLEBUFFER: u32 = 0x0000_0001;
pub const PFD_DRAW_TO_WINDOW: u32 = 0x0000_0004;
pub const PFD_SUPPORT_OPENGL: u32 = 0x0000_0020;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PIXELFORMATDESCRIPTOR {
    pub nSize: u16,
    pub nVersion: u16,
    pub dwFlags: u32,
    pub iPixelType: u8,
    pub cColorBits: u8,
    pub cRedBits: u8,
    pub cRedShift: u8,
    pub cGreenBits: u8,
    pub cGreenShift: u8,
    pub cBlueBits: u8,
    pub cBlueShift: u8,
    pub cAlphaBits: u8,
    pub cAlphaShift: u8,
    pub cAccumBits: u8,
    pub cAccumRedBits: u8,
    pub cAccumGreenBits: u8,
    pub cAccumBlueBits: u8,
    pub cAccumAlphaBits: u8,
    pub cDepthBits: u8,
    pub cStencilBits: u8,
    pub cAuxBuffers: u8,
    pub iLayerType: u8,
    pub bReserved: u8,
    pub dwLayerMask: u32,
    pub dwVisibleMask: u32,
    pub dwDamageMask: u32,
}

pub const PFD_SIZE: u16 = core::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PROCESS_MEMORY_COUNTERS_EX {
    pub cb: u32,
    pub PageFaultCount: u32,
    pub PeakWorkingSetSize: usize,
    pub WorkingSetSize: usize,
    pub QuotaPeakPagedPoolUsage: usize,
    pub QuotaPagedPoolUsage: usize,
    pub QuotaPeakNonPagedPoolUsage: usize,
    pub QuotaNonPagedPoolUsage: usize,
    pub PagefileUsage: usize,
    pub PeakPagefileUsage: usize,
    pub PrivateUsage: usize,
}

#[repr(C)]
pub struct MINIDUMP_EXCEPTION_INFORMATION {
    pub ThreadId: u32,
    pub ExceptionPointers: *mut c_void,
    pub ClientPointers: BOOL,
}

/// `LPTOP_LEVEL_EXCEPTION_FILTER`
pub type TopLevelFilter = unsafe extern "system" fn(*mut c_void) -> i32;

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HMODULE;
    pub fn GetModuleFileNameW(module: HMODULE, buf: *mut u16, size: u32) -> u32;
    pub fn LoadLibraryW(name: *const u16) -> HMODULE;
    pub fn GetProcAddress(module: HMODULE, name: *const u8) -> FARPROC;
    pub fn DisableThreadLibraryCalls(module: HMODULE) -> BOOL;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old: *mut u32) -> BOOL;
    pub fn QueryPerformanceCounter(value: *mut i64) -> BOOL;
    pub fn QueryPerformanceFrequency(value: *mut i64) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetLocalTime(st: *mut SYSTEMTIME);
    pub fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    pub fn CreateWaitableTimerExW(attr: *mut c_void, name: *const u16, flags: u32, access: u32) -> HANDLE;
    pub fn SetWaitableTimer(
        timer: HANDLE,
        due: *const i64,
        period: i32,
        completion: *mut c_void,
        arg: *mut c_void,
        resume: BOOL,
    ) -> BOOL;
    pub fn WaitForSingleObject(handle: HANDLE, ms: u32) -> u32;
    pub fn SetUnhandledExceptionFilter(filter: Option<TopLevelFilter>) -> Option<TopLevelFilter>;
    pub fn K32EnumProcessModules(process: HANDLE, modules: *mut HMODULE, cb: u32, needed: *mut u32) -> BOOL;
    pub fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut PROCESS_MEMORY_COUNTERS_EX, cb: u32) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn SetProcessDPIAware() -> BOOL;
}

#[link(name = "winmm")]
extern "system" {
    pub fn timeBeginPeriod(ms: u32) -> u32;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn DescribePixelFormat(hdc: HDC, format: i32, bytes: u32, pfd: *mut PIXELFORMATDESCRIPTOR) -> i32;
}

/// `MiniDumpWriteDump` (loaded dynamically from dbghelp.dll).
pub type MiniDumpWriteDumpFn = unsafe extern "system" fn(
    process: HANDLE,
    pid: u32,
    file: HANDLE,
    dump_type: u32,
    exception: *const MINIDUMP_EXCEPTION_INFORMATION,
    user_stream: *const c_void,
    callback: *const c_void,
) -> BOOL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_format_descriptor_has_win32_layout() {
        assert_eq!(PFD_SIZE, 40);
        assert_eq!(core::mem::offset_of!(PIXELFORMATDESCRIPTOR, cDepthBits), 23);
        assert_eq!(core::mem::offset_of!(PIXELFORMATDESCRIPTOR, cStencilBits), 24);
        assert_eq!(core::mem::offset_of!(PIXELFORMATDESCRIPTOR, dwLayerMask), 28);
    }
}
