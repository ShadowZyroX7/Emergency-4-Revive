//! "Known missing" file cache.
//!
//! EM4's virtual file system probes every file in several folders
//! (`Data/lang/any`, `Data/lang/en`, the mod, `Data`) and asks again and
//! again for the same non-existent files – ~78 000 failed opens per map load
//! with AUBMP, plus ~670 per second while playing (script lookups).
//!
//! After a read-only open of a path failed with "not found", further
//! read-only opens of exactly that path are answered immediately. Any open
//! that may create or modify the file removes the entry again, so files the
//! game writes itself (saves, screenshots, logs) are never hidden.

use crate::ffi::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, OPEN_EXISTING, WRITE_ACCESS_MASK};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Upper bound so a pathological mod cannot grow the cache without limit.
const MAX_ENTRIES: usize = 200_000;

/// Normalised cache key: lower case, backslashes, no doubled separators.
pub fn normalize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_sep = false;
    for c in path.chars() {
        let c = if c == '/' { '\\' } else { c };
        if c == '\\' {
            if prev_sep && !out.is_empty() && out != "\\" {
                continue;
            }
            prev_sep = true;
        } else {
            prev_sep = false;
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Only plain "open an existing file for reading" requests are cacheable.
pub fn is_read_only_open(access: u32, disposition: u32) -> bool {
    disposition == OPEN_EXISTING && access & WRITE_ACCESS_MASK == 0
}

pub fn is_not_found(error: u32) -> bool {
    error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
}

#[derive(Default)]
pub struct MissingCache {
    // Created on first use: HashMap::new() is not const (random hash seed).
    entries: OnceLock<Mutex<HashMap<String, u32>>>,
}

impl MissingCache {
    pub const fn new() -> Self {
        Self { entries: OnceLock::new() }
    }

    fn map(&self) -> &Mutex<HashMap<String, u32>> {
        self.entries.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The remembered error code if `key` is known to be missing.
    pub fn lookup(&self, key: &str) -> Option<u32> {
        self.map().lock().ok()?.get(key).copied()
    }

    pub fn remember(&self, key: String, error: u32) {
        if let Ok(mut m) = self.map().lock() {
            if m.len() < MAX_ENTRIES {
                m.insert(key, error);
            }
        }
    }

    pub fn forget(&self, key: &str) {
        if let Ok(mut m) = self.map().lock() {
            m.remove(key);
        }
    }

    pub fn len(&self) -> usize {
        self.map().lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(
            normalize(r"C:\Spiel\mods/AUBMP//Textures\Detailpoly/DETAIL.tga"),
            r"c:\spiel\mods\aubmp\textures\detailpoly\detail.tga"
        );
        assert_eq!(normalize("Data/lang/any/detail.tga"), r"data\lang\any\detail.tga");
        // UNC prefix keeps its double backslash.
        assert_eq!(normalize(r"\\server\share"), r"\\server\share");
    }

    #[test]
    fn only_read_only_opens_are_cacheable() {
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_ALWAYS: u32 = 2;
        const OPEN_ALWAYS: u32 = 4;
        assert!(is_read_only_open(GENERIC_READ, OPEN_EXISTING));
        assert!(!is_read_only_open(GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING));
        assert!(!is_read_only_open(GENERIC_READ, OPEN_ALWAYS));
        assert!(!is_read_only_open(GENERIC_WRITE, CREATE_ALWAYS));
        assert!(!is_read_only_open(0x4 /* FILE_APPEND_DATA */, OPEN_EXISTING));
    }

    #[test]
    fn remember_lookup_forget() {
        let c = MissingCache::new();
        let k = normalize("Data/x.tga");
        assert_eq!(c.lookup(&k), None);
        c.remember(k.clone(), ERROR_FILE_NOT_FOUND);
        assert_eq!(c.lookup(&normalize(r"DATA\X.TGA")), Some(ERROR_FILE_NOT_FOUND));
        c.forget(&k);
        assert_eq!(c.lookup(&k), None);
        assert!(is_not_found(ERROR_PATH_NOT_FOUND));
        assert!(!is_not_found(5 /* access denied */));
    }
}
