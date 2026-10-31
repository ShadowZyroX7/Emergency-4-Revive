//! Address -> module (and, for system DLLs, function name) lookup.

use em4r_core::ffi::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub type Exports = Arc<Vec<(u32, String)>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    pub base: usize,
    pub end: usize,
    /// Display name, e.g. `vision71.dll` or `opengl32.dll (ReShade)`.
    pub name: String,
    /// Code written by the game developers (callers worth attributing to).
    pub game_code: bool,
    /// Named exports, only for non-game modules (to name Windows functions).
    pub exports: Option<Exports>,
}

impl ModEntry {
    /// `ntdll.dll!NtCreateFile` for system DLLs, otherwise just the module.
    pub fn symbol(&self, addr: usize) -> String {
        if let Some(ex) = &self.exports {
            if let Some(name) = em4r_core::pe::symbolize(ex, (addr - self.base) as u32) {
                return format!("{}!{}", self.name, name);
            }
        }
        self.name.clone()
    }
}

/// Modules in the game folder that are *not* game code.
const NOT_GAME_CODE: &[&str] = &[
    "msvcr71.dll",
    "msvcp71.dll",
    "dinput8.dll",
    "opengl32.dll",
    "dbghelp.dll",
    "unicows.dll",
];

#[derive(Debug, Default, Clone)]
pub struct ModuleMap {
    entries: Vec<ModEntry>,
}

/// Export tables survive between snapshots (keyed by module base).
#[derive(Default)]
pub struct ExportCache {
    by_base: HashMap<usize, Exports>,
}

impl ModuleMap {
    pub fn from_entries(mut entries: Vec<ModEntry>) -> Self {
        entries.sort_by_key(|e| e.base);
        Self { entries }
    }

    /// Snapshot of all modules currently loaded in this process.
    pub fn snapshot(game_dir: &Path, cache: &mut ExportCache) -> Self {
        let mut handles = vec![core::ptr::null_mut(); 1024];
        let mut needed = 0u32;
        // SAFETY: valid buffer and sizes.
        let ok = unsafe {
            K32EnumProcessModules(
                GetCurrentProcess(),
                handles.as_mut_ptr(),
                (handles.len() * core::mem::size_of::<HMODULE>()) as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return Self::default();
        }
        let count = (needed as usize / core::mem::size_of::<HMODULE>()).min(handles.len());
        let dir = game_dir.to_string_lossy().to_ascii_lowercase();
        let mut entries = Vec::with_capacity(count);
        for &m in &handles[..count] {
            let mut info = MODULEINFO { lpBaseOfDll: core::ptr::null_mut(), SizeOfImage: 0, EntryPoint: core::ptr::null_mut() };
            // SAFETY: valid out struct.
            let ok = unsafe {
                K32GetModuleInformation(GetCurrentProcess(), m, &mut info, core::mem::size_of::<MODULEINFO>() as u32)
            };
            if ok == 0 {
                continue;
            }
            let Some(path) = em4r_core::util::module_path(m) else { continue };
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let lower = file.to_ascii_lowercase();
            let in_game_dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_ascii_lowercase() == dir)
                .unwrap_or(false);
            let name = match (in_game_dir, lower.as_str()) {
                (true, "opengl32.dll") => format!("{file} (ReShade)"),
                (true, "dinput8.dll") => format!("{file} (EM4 Revive)"),
                _ => file,
            };
            let game_code = in_game_dir && !NOT_GAME_CODE.contains(&lower.as_str());
            let base = info.lpBaseOfDll as usize;
            let exports = (!game_code).then(|| {
                cache
                    .by_base
                    // SAFETY: `m` is a loaded module.
                    .entry(base)
                    .or_insert_with(|| Arc::new(unsafe { em4r_core::pe::exports(m) }))
                    .clone()
            });
            entries.push(ModEntry { base, end: base + info.SizeOfImage as usize, name, game_code, exports });
        }
        Self::from_entries(entries)
    }

    pub fn lookup(&self, addr: usize) -> Option<&ModEntry> {
        let i = self.entries.partition_point(|e| e.base <= addr);
        let e = self.entries.get(i.checked_sub(1)?)?;
        (addr < e.end).then_some(e)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(base: usize, end: usize, name: &str, game_code: bool) -> ModEntry {
        ModEntry { base, end, name: name.into(), game_code, exports: None }
    }

    fn map() -> ModuleMap {
        ModuleMap::from_entries(vec![
            entry(0x1000_0000, 0x1029_0000, "vision71.dll", true),
            entry(0x0040_0000, 0x00E0_0000, "Em4.exe", true),
            entry(0x7700_0000, 0x7710_0000, "ntdll.dll", false),
        ])
    }

    #[test]
    fn lookup_hits_and_misses() {
        let m = map();
        assert_eq!(m.lookup(0x008C_1199).unwrap().name, "Em4.exe");
        assert_eq!(m.lookup(0x1000_0000).unwrap().name, "vision71.dll");
        assert!(m.lookup(0x1029_0000).is_none()); // end is exclusive
        assert!(m.lookup(0x0000_1000).is_none());
        assert!(m.lookup(0x5000_0000).is_none());
        assert_eq!(m.lookup(0x770F_FFFF).unwrap().name, "ntdll.dll");
    }

    #[test]
    fn snapshot_names_ntdll_functions() {
        let mut cache = ExportCache::default();
        let m = ModuleMap::snapshot(Path::new(r"C:\does\not\exist"), &mut cache);
        assert!(m.len() > 3);
        let addr = em4r_core::util::loaded_proc("ntdll.dll", "NtClose").unwrap();
        let e = m.lookup(addr).unwrap();
        assert!(e.name.eq_ignore_ascii_case("ntdll.dll"));
        assert!(!e.game_code);
        assert!(e.symbol(addr).ends_with("!NtClose"), "{}", e.symbol(addr));
        // Second snapshot reuses the cached export tables.
        let before = cache.by_base.len();
        let _ = ModuleMap::snapshot(Path::new(r"C:\does\not\exist"), &mut cache);
        assert_eq!(cache.by_base.len(), before);
    }
}
