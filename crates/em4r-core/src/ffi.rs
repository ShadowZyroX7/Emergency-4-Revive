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
    pub fn LoadLibraryExW(name: *const u16, file: HANDLE, flags: u32) -> HMODULE;
    pub fn FreeLibrary(module: HMODULE) -> BOOL;
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
    pub fn K32GetModuleInformation(process: HANDLE, module: HMODULE, info: *mut MODULEINFO, cb: u32) -> BOOL;
    pub fn OpenThread(access: u32, inherit: BOOL, thread_id: u32) -> HANDLE;
    pub fn SuspendThread(thread: HANDLE) -> u32;
    pub fn ResumeThread(thread: HANDLE) -> u32;
    pub fn GetThreadContext(thread: HANDLE, context: *mut CONTEXT) -> BOOL;
    pub fn VirtualQuery(addr: *const c_void, info: *mut MEMORY_BASIC_INFORMATION, len: usize) -> usize;
    pub fn ReadProcessMemory(process: HANDLE, addr: *const c_void, buf: *mut c_void, size: usize, read: *mut usize) -> BOOL;
    pub fn CloseHandle(handle: HANDLE) -> BOOL;
    pub fn Sleep(ms: u32);
    pub fn FlushInstructionCache(process: HANDLE, addr: *const c_void, size: usize) -> BOOL;
    pub fn TerminateProcess(process: HANDLE, exit_code: u32) -> BOOL;
    pub fn GetLastError() -> u32;
    pub fn SetLastError(code: u32);
}

pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const OPEN_EXISTING: u32 = 3;
/// Access bits that modify a file (GENERIC_WRITE, GENERIC_ALL, FILE_WRITE_DATA,
/// FILE_APPEND_DATA, DELETE, WRITE_DAC, WRITE_OWNER, FILE_WRITE_ATTRIBUTES/EA).
pub const WRITE_ACCESS_MASK: u32 = 0x4000_0000 | 0x1000_0000 | 0x2 | 0x4 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x100 | 0x10;

pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_GET_CONTEXT: u32 = 0x0008;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;
pub const INVALID_HANDLE_VALUE: HANDLE = usize::MAX as HANDLE;
pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MODULEINFO {
    pub lpBaseOfDll: *mut c_void,
    pub SizeOfImage: u32,
    pub EntryPoint: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MEMORY_BASIC_INFORMATION {
    pub BaseAddress: *mut c_void,
    pub AllocationBase: *mut c_void,
    pub AllocationProtect: u32,
    #[cfg(target_pointer_width = "64")]
    pub PartitionId: u16,
    pub RegionSize: usize,
    pub State: u32,
    pub Protect: u32,
    pub Type: u32,
}

/// x86 `CONTEXT` (716 bytes). Only the control registers are used.
#[cfg(target_arch = "x86")]
#[repr(C, align(4))]
pub struct CONTEXT {
    pub raw: [u32; 179],
}

#[cfg(target_arch = "x86")]
impl CONTEXT {
    pub const CONTEXT_CONTROL: u32 = 0x0001_0001;
    pub fn control() -> Self {
        let mut c = CONTEXT { raw: [0; 179] };
        c.raw[0] = Self::CONTEXT_CONTROL;
        c
    }
    pub fn eip(&self) -> u32 {
        self.raw[0xB8 / 4]
    }
    pub fn esp(&self) -> u32 {
        self.raw[0xC4 / 4]
    }
}

/// Placeholder so the crate still type-checks on non-x86 hosts.
#[cfg(not(target_arch = "x86"))]
#[repr(C)]
pub struct CONTEXT {
    pub raw: [u8; 0],
}

pub type HWND = *mut c_void;

#[link(name = "user32")]
extern "system" {
    pub fn SetProcessDPIAware() -> BOOL;
    pub fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: *mut c_void,
        instance: HMODULE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
}

pub const WS_POPUP: u32 = 0x8000_0000;

#[link(name = "winmm")]
extern "system" {
    pub fn timeBeginPeriod(ms: u32) -> u32;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn DescribePixelFormat(hdc: HDC, format: i32, bytes: u32, pfd: *mut PIXELFORMATDESCRIPTOR) -> i32;
    pub fn ChoosePixelFormat(hdc: HDC, pfd: *const PIXELFORMATDESCRIPTOR) -> i32;
    pub fn SetPixelFormat(hdc: HDC, format: i32, pfd: *const PIXELFORMATDESCRIPTOR) -> BOOL;
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

    #[cfg(target_arch = "x86")]
    #[test]
    fn x86_struct_sizes() {
        assert_eq!(core::mem::size_of::<CONTEXT>(), 716);
        assert_eq!(core::mem::size_of::<MEMORY_BASIC_INFORMATION>(), 28);
        assert_eq!(core::mem::size_of::<MODULEINFO>(), 12);
    }
}
