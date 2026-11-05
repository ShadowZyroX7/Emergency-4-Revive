//! Windows integration: processes, elevation, dialogs, uninstall entry, shortcuts.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_YESNO, SW_HIDE,
};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\EM4Revive";

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn is_process_running(exe_name: &str) -> bool {
    // SAFETY: plain Toolhelp enumeration on a local, zero-initialised entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = core::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more && !found {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            found = String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe_name);
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Wait until the process has ended (no-op if it is already gone).
pub fn wait_for_pid(pid: u32, timeout_ms: u32) {
    // SAFETY: the handle is checked and closed; the wait has no memory effects.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            WaitForSingleObject(handle, timeout_ms);
            CloseHandle(handle);
        }
    }
}

/// Run this exe elevated (UAC prompt) with `args` and wait for it.
pub fn run_self_elevated(args: &str) -> io::Result<()> {
    let exe = wide(std::env::current_exe()?);
    let verb = wide("runas");
    let params = wide(args);
    // SAFETY: all pointers refer to NUL-terminated buffers that outlive the call.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = core::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = exe.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(if GetLastError() == ERROR_CANCELLED {
                io::Error::new(io::ErrorKind::PermissionDenied, "Die Administrator-Abfrage wurde abgelehnt.")
            } else {
                io::Error::last_os_error()
            });
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if code != 0 {
            return Err(io::Error::other(format!("Der Vorgang mit Administratorrechten ist fehlgeschlagen (Code {code}).")));
        }
    }
    Ok(())
}

pub fn quote_dir(dir: &Path) -> String {
    format!("\"{}\"", dir.display().to_string().trim_end_matches('\\'))
}

fn message_box(text: &str, flags: u32) -> i32 {
    let (text, title) = (wide(text), wide("EM4 Revive Setup"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { MessageBoxW(core::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
}

pub fn error_box(text: &str) {
    message_box(text, MB_ICONERROR);
}

pub fn info_box(text: &str) {
    message_box(text, MB_ICONINFORMATION);
}

pub fn confirm_box(text: &str) -> bool {
    message_box(text, MB_YESNO | MB_ICONQUESTION) == IDYES
}

/// The entry under Settings > Apps. Lives in HKCU, needs no elevation.
pub fn register_uninstall_entry(dir: &Path, uninstaller: &Path, version: &str) -> io::Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
    key.set_value("DisplayName", &"EM4 Revive")?;
    key.set_value("DisplayVersion", &version)?;
    key.set_value("Publisher", &env!("CARGO_PKG_AUTHORS"))?;
    key.set_value("InstallLocation", &dir.display().to_string())?;
    key.set_value("DisplayIcon", &dir.join("EM4Revive.exe").display().to_string())?;
    key.set_value(
        "UninstallString",
        &format!("\"{}\" --uninstall --dir {}", uninstaller.display(), quote_dir(dir)),
    )?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)
}

pub fn remove_uninstall_entry() {
    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY);
}

/// PowerShell gets its values through the environment, so paths cannot break
/// out of the script text.
fn powershell(script: &str, target: &Path, work_dir: &Path) -> io::Result<()> {
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("EM4R_TARGET", target)
        .env("EM4R_WORKDIR", work_dir)
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Verknüpfung konnte nicht erstellt werden."))
    }
}

pub fn create_shortcuts(dir: &Path, desktop: bool) -> io::Result<()> {
    let folders = if desktop { "'Programs','Desktop'" } else { "'Programs'" };
    let script = format!(
        "$ErrorActionPreference='Stop'; $s=New-Object -ComObject WScript.Shell; \
         foreach($f in {folders}){{ \
           $l=$s.CreateShortcut((Join-Path ([Environment]::GetFolderPath($f)) 'EM4 Revive.lnk')); \
           $l.TargetPath=$env:EM4R_TARGET; $l.WorkingDirectory=$env:EM4R_WORKDIR; $l.Save() }}"
    );
    powershell(&script, &dir.join("EM4Revive.exe"), dir)
}

pub fn remove_shortcuts(dir: &Path) {
    let script = "foreach($f in 'Programs','Desktop'){ \
        Remove-Item (Join-Path ([Environment]::GetFolderPath($f)) 'EM4 Revive.lnk') -ErrorAction SilentlyContinue }";
    let _ = powershell(script, &dir.join("EM4Revive.exe"), dir);
}

/// A running exe cannot delete itself, so a short-lived cmd does it afterwards.
pub fn delete_after_exit(path: &Path) {
    let _ = Command::new("cmd.exe")
        .raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", path.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
