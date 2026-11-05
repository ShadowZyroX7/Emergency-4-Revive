//! EM4 Revive Setup.
//!
//! ```text
//! (no arguments)                        setup window
//! --update --dir D [--wait-pid P]       from the launcher: update silently, restart the launcher
//! --uninstall --dir D [--quiet]         uninstall (Windows Settings > Apps)
//! --files-only / --uninstall-files --dir D    internal: files only, runs elevated
//! ```
#![windows_subsystem = "windows"]

mod files;
mod gui;
mod steam;
mod system;

use files::Payload;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod payload {
    include!(concat!(env!("OUT_DIR"), "/payload.rs"));
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const PAYLOAD: Payload<'static> = Payload {
    dll: payload::DINPUT8,
    launcher: payload::LAUNCHER,
    default_config: payload::DEFAULT_CONFIG,
    version: VERSION,
};

fn ensure_game_closed() -> io::Result<()> {
    if system::is_process_running("Em4.exe") {
        return Err(io::Error::other("Emergency 4 läuft noch. Bitte beende das Spiel und versuche es erneut."));
    }
    Ok(())
}

fn install_files(dir: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    files::install(dir, &PAYLOAD, Some(&exe))
}

/// Copy the files, asking for administrator rights only if the folder needs
/// it. Everything user-specific stays in the unelevated caller.
pub fn perform_install(dir: &Path) -> io::Result<()> {
    ensure_game_closed()?;
    if files::is_writable(dir) {
        install_files(dir)?;
    } else {
        system::run_self_elevated(&format!("--files-only --dir {}", system::quote_dir(dir)))?;
    }
    system::register_uninstall_entry(dir, &files::uninstaller_path(dir), VERSION)
}

pub fn start_launcher(dir: &Path, extra_args: &[&str]) -> io::Result<()> {
    Command::new(dir.join("EM4Revive.exe")).args(extra_args).current_dir(dir).spawn().map(|_| ())
}

fn run_update(dir: &Path, wait_pid: Option<u32>) -> ExitCode {
    // Launchers up to 0.2.2 can start the setup twice. Two copies would fight
    // over the same files, and the loser would report a failed update.
    if !system::claim_update_slot() {
        return ExitCode::SUCCESS;
    }
    if let Some(pid) = wait_pid {
        system::wait_for_pid(pid, 30_000);
    }
    match perform_install(dir) {
        Ok(()) => {
            let _ = start_launcher(dir, &[]);
            ExitCode::SUCCESS
        }
        Err(e) => {
            system::error_box(&format!("Das Update konnte nicht installiert werden:\n\n{e}"));
            // The old version still works; do not retry right away.
            let _ = start_launcher(dir, &["--no-update-check"]);
            ExitCode::FAILURE
        }
    }
}

fn run_uninstall(dir: &Path, quiet: bool) -> ExitCode {
    if !quiet && !system::confirm_box("EM4 Revive wirklich entfernen?\n\nDas Spiel selbst bleibt unverändert.") {
        return ExitCode::SUCCESS;
    }
    let result = ensure_game_closed().and_then(|()| {
        if files::is_writable(dir) {
            files::uninstall(dir)
        } else {
            system::run_self_elevated(&format!("--uninstall-files --dir {}", system::quote_dir(dir)))
        }
    });
    match result {
        Ok(()) => {
            system::remove_shortcuts(dir);
            system::remove_uninstall_entry();
            if let Ok(exe) = std::env::current_exe() {
                if exe.starts_with(dir) {
                    system::delete_after_exit(&exe);
                }
            }
            if !quiet {
                system::info_box("EM4 Revive wurde entfernt.\nDeine em4revive.toml und Crash-Dumps liegen weiter im Spielordner.");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if !quiet {
                system::error_box(&format!("EM4 Revive konnte nicht entfernt werden:\n\n{e}"));
            }
            ExitCode::FAILURE
        }
    }
}

fn exit_code(result: io::Result<()>) -> ExitCode {
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let dir = value("--dir").map(PathBuf::from);
    let wait_pid = value("--wait-pid").and_then(|p| p.parse().ok());

    match (dir, args.first().map(String::as_str)) {
        (Some(d), Some("--files-only")) => exit_code(install_files(&d)),
        (Some(d), Some("--uninstall-files")) => exit_code(files::uninstall(&d)),
        (Some(d), Some("--update")) => run_update(&d, wait_pid),
        (Some(d), Some("--uninstall")) => run_uninstall(&d, has("--quiet")),
        _ => gui::run(),
    }
}
