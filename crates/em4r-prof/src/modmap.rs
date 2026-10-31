//! Address -> module lookup.

use em4r_core::ffi::*;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    pub base: usize,
    pub end: usize,
    /// Display name, e.g. `vision71.dll` or `opengl32.dll (ReShade)`.
    pub name: String,
    /// Code written by the game developers (callers worth attributing to).
    pub game_code: bool,
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

impl ModuleMap {
    pub fn from_entries(mut entries: Vec<ModEntry>) -> Self {
        entries.sort_by_key(|e| e.base);
        Self { entries }
    }

    /// Snapshot of all modules currently loaded in this process.
    pub fn snapshot(game_dir: &Path) -> Self {
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
            let base = info.lpBaseOfDll as usize;
            entries.push(ModEntry {
                base,
                end: base + info.SizeOfImage as usize,
                name,
                game_code: in_game_dir && !NOT_GAME_CODE.contains(&lower.as_str()),
            });
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

    fn map() -> ModuleMap {
        ModuleMap::from_entries(vec![
            ModEntry { base: 0x1000_0000, end: 0x1029_0000, name: "vision71.dll".into(), game_code: true },
            ModEntry { base: 0x0040_0000, end: 0x00E0_0000, name: "Em4.exe".into(), game_code: true },
            ModEntry { base: 0x7700_0000, end: 0x7710_0000, name: "ntdll.dll".into(), game_code: false },
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
    fn snapshot_of_test_process_contains_ntdll() {
        let m = ModuleMap::snapshot(Path::new(r"C:\does\not\exist"));
        assert!(m.len() > 3);
        let addr = em4r_core::util::loaded_proc("ntdll.dll", "NtClose").unwrap();
        let e = m.lookup(addr).unwrap();
        assert!(e.name.eq_ignore_ascii_case("ntdll.dll"));
        assert!(!e.game_code);
    }
}
