//! Core infrastructure for EM4 Revive: Win32 bindings, configuration,
//! logging, import-table (IAT) hooking and the crash handler.

pub mod config;
pub mod counters;
pub mod crash;
pub mod ffi;
pub mod hook;
pub mod launch;
pub mod log;
pub mod patch;
pub mod pe;
pub mod util;

pub use config::Config;
