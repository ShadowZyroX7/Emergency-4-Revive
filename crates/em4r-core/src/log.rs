//! Tiny file logger (`em4revive.log`). Every line is flushed immediately so
//! the log is complete even if the game crashes right after.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static FILE: Mutex<Option<File>> = Mutex::new(None);
static DEBUG: AtomicBool = AtomicBool::new(false);
/// Warnings produced before the log file was open (e.g. while parsing config).
static DEFERRED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

pub fn init(path: &Path, debug: bool) -> std::io::Result<()> {
    DEBUG.store(debug, Ordering::Relaxed);
    let file = File::create(path)?;
    if let Ok(mut g) = FILE.lock() {
        *g = Some(file);
    }
    let pending = DEFERRED.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
    for w in pending {
        write(Level::Warn, &w);
    }
    Ok(())
}

pub fn defer_warning(msg: String) {
    if let Ok(mut d) = DEFERRED.lock() {
        d.push(msg);
    }
}

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

pub fn write(level: Level, msg: &str) {
    if level == Level::Debug && !debug_enabled() {
        return;
    }
    let t = crate::util::local_time();
    let tag = match level {
        Level::Debug => "DEBUG",
        Level::Info => "INFO ",
        Level::Warn => "WARN ",
        Level::Error => "ERROR",
    };
    let line = format!(
        "{:02}:{:02}:{:02}.{:03} [{:>5}] {} | {}\r\n",
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds,
        // SAFETY: trivial Win32 call.
        unsafe { crate::ffi::GetCurrentThreadId() },
        tag,
        msg
    );
    // try_lock-free path is not needed: logging happens off the hot path.
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}

#[macro_export]
macro_rules! debug { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Debug, &format!($($a)*)) } }
#[macro_export]
macro_rules! info { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Info, &format!($($a)*)) } }
#[macro_export]
macro_rules! warn { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Warn, &format!($($a)*)) } }
#[macro_export]
macro_rules! error { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Error, &format!($($a)*)) } }
