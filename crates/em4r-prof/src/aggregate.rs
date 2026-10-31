//! Turning raw samples (instruction pointer + stack copy) into statistics.

use crate::modmap::ModuleMap;
use em4r_core::counters::Phase;
use std::collections::HashMap;
use std::fmt::Write;

/// Hotspot granularity inside game code (bytes).
const HOTSPOT_BUCKET: usize = 0x40;

#[derive(Default)]
pub struct Aggregate {
    pub total: HashMap<Phase, u64>,
    pub by_module: HashMap<(Phase, String), u64>,
    /// Non-game functions by name, e.g. `ntdll.dll!NtCreateFile`.
    pub by_function: HashMap<(Phase, String), u64>,
    pub hotspots: HashMap<(Phase, String, usize), u64>,
    /// Time spent outside game code, attributed to the nearest verified
    /// game-code return address on the stack.
    pub callers: HashMap<(Phase, String, String), u64>,
}

impl Aggregate {
    /// `code_before(addr)` returns the 7 bytes before `addr` (if readable);
    /// used to accept only stack values that follow a `call` instruction.
    pub fn add(
        &mut self,
        phase: Phase,
        eip: usize,
        stack: &[u32],
        mods: &ModuleMap,
        code_before: impl Fn(usize) -> Option<[u8; 7]>,
    ) {
        *self.total.entry(phase).or_default() += 1;
        let Some(leaf) = mods.lookup(eip) else {
            *self.by_module.entry((phase, "(unbekannt)".into())).or_default() += 1;
            return;
        };
        *self.by_module.entry((phase, leaf.name.clone())).or_default() += 1;
        if leaf.game_code {
            let bucket = (eip - leaf.base) & !(HOTSPOT_BUCKET - 1);
            *self.hotspots.entry((phase, leaf.name.clone(), bucket)).or_default() += 1;
            return;
        }
        let func = leaf.symbol(eip);
        *self.by_function.entry((phase, func.clone())).or_default() += 1;
        let caller = stack
            .iter()
            .map(|&v| v as usize)
            .filter_map(|v| mods.lookup(v).filter(|m| m.game_code).map(|m| (m, v)))
            .find(|&(_, v)| code_before(v).is_some_and(|b| em4r_core::pe::preceded_by_call(&b)))
            .map(|(m, v)| format!("{}+0x{:X}", m.name, v - m.base))
            .unwrap_or_else(|| "(kein Spielcode gefunden)".into());
        *self.callers.entry((phase, func, caller)).or_default() += 1;
    }

    pub fn samples(&self) -> u64 {
        self.total.values().sum()
    }

    /// All raw data as CSV for offline analysis (`em4r-re fold`):
    /// `kind;phase;module;offset_or_function;caller;count`.
    pub fn raw_csv(&self) -> String {
        let phase = |p: &Phase| if *p == Phase::Playing { "spiel" } else { "laden" };
        let mut out = String::from("art;phase;modul;offset_oder_funktion;aufrufer;anzahl\n");
        for (p, n) in &self.total {
            out.push_str(&format!("gesamt;{};;;;{n}\n", phase(p)));
        }
        for ((p, m, off), n) in &self.hotspots {
            out.push_str(&format!("hotspot;{};{m};0x{off:X};;{n}\n", phase(p)));
        }
        for ((p, f, c), n) in &self.callers {
            out.push_str(&format!("aufruf;{};;{f};{c};{n}\n", phase(p)));
        }
        out
    }

    /// Human-readable report section for one phase.
    pub fn render_phase(&self, phase: Phase, out: &mut String) {
        let total = *self.total.get(&phase).unwrap_or(&0);
        let _ = writeln!(out, "\n=== {} — {} Stichproben ===", phase.label(), total);
        if total == 0 {
            let _ = writeln!(out, "  (keine Daten)");
            return;
        }
        let pct = |n: u64| n as f64 * 100.0 / total as f64;
        let in_phase = |m: &HashMap<(Phase, String), u64>| {
            m.iter().filter(|((p, _), _)| *p == phase).map(|((_, k), n)| (k.clone(), *n)).collect::<Vec<_>>()
        };

        let _ = writeln!(out, "\n  Wo die Zeit verbracht wird (Modul):");
        for (name, n) in top(in_phase(&self.by_module).into_iter(), 15) {
            let _ = writeln!(out, "    {:5.1} %  {:<34} {}", pct(n), name, describe(&name));
        }

        let _ = writeln!(out, "\n  Windows-/Treiber-Funktionen (außerhalb des Spielcodes):");
        for (name, n) in top(in_phase(&self.by_function).into_iter(), 15) {
            let _ = writeln!(out, "    {:5.1} %  {name}", pct(n));
        }

        let _ = writeln!(out, "\n  … aufgerufen von (geprüfte Rücksprungadresse im Spielcode):");
        let rows = top(
            self.callers
                .iter()
                .filter(|((p, _, _), _)| *p == phase)
                .map(|((_, func, caller), n)| (format!("{func:<44} <- {caller}"), *n)),
            15,
        );
        if rows.is_empty() {
            let _ = writeln!(out, "    (keine)");
        }
        for (label, n) in rows {
            let _ = writeln!(out, "    {:5.1} %  {label}", pct(n));
        }

        let _ = writeln!(out, "\n  Hotspots im Spielcode (Adressen für die weitere Analyse):");
        for (label, n) in top(
            self.hotspots
                .iter()
                .filter(|((p, _, _), _)| *p == phase)
                .map(|((_, m, off), n)| (format!("{m}+0x{off:X}"), *n)),
            20,
        ) {
            let _ = writeln!(out, "    {:5.1} %  {label}", pct(n));
        }
    }
}

fn top(items: impl Iterator<Item = (String, u64)>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<_> = items.collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// Short German explanation for well-known modules.
pub fn describe(module: &str) -> &'static str {
    let m = module.to_ascii_lowercase();
    match () {
        _ if m.starts_with("em4.exe") => "Spiellogik, Skripte, KI, Simulation",
        _ if m.starts_with("vision71") => "Render-Engine (Vision 7.1)",
        _ if m.starts_with("vbase71") => "Engine-Basis (Dateien, Fenster, Eingabe)",
        _ if m.contains("reshade") => "ReShade (Post-Processing)",
        _ if m.contains("em4 revive") => "EM4 Revive (diese Mod)",
        _ if m.starts_with("nvoglv") || m.starts_with("atioglxx") || (m.starts_with("ig") && m.contains("icd")) => {
            "Grafiktreiber (OpenGL)"
        }
        _ if m.starts_with("opengl32") => "Windows-OpenGL",
        _ if m.starts_with("msvcr71") || m.starts_with("msvcp71") => "C-Laufzeit (Speicher, Dateien, Strings)",
        _ if m.starts_with("ntdll") || m.starts_with("kernel") => "Windows-Kern (Dateien, Speicher, Warten)",
        _ if m.starts_with("win32u") => "Windows-Grafik-Systemaufrufe (u.a. Bildwechsel)",
        _ if m.starts_with("gdi") || m.starts_with("user32") => "Windows-Grafik/Fenster",
        _ if m.starts_with("dsound") || m.starts_with("hapi") => "Audio",
        _ if m.starts_with("foundation") => "Havok Physik-Basis",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modmap::{ModEntry, ModuleMap};
    use std::sync::Arc;

    fn map() -> ModuleMap {
        ModuleMap::from_entries(vec![
            ModEntry { base: 0x0040_0000, end: 0x00E0_0000, name: "Em4.exe".into(), game_code: true, exports: None },
            ModEntry {
                base: 0x7700_0000,
                end: 0x7710_0000,
                name: "ntdll.dll".into(),
                game_code: false,
                exports: Some(Arc::new(vec![(0x1000, "NtCreateFile".into()), (0x2000, "NtClose".into())])),
            },
            ModEntry {
                base: 0x6EB3_0000,
                end: 0x6EBB_0000,
                name: "dinput8.dll (EM4 Revive)".into(),
                game_code: false,
                exports: None,
            },
        ])
    }

    /// Fake code memory: only 0x0070_0000 is preceded by a `call rel32`.
    fn code(addr: usize) -> Option<[u8; 7]> {
        Some(if addr == 0x0070_0000 { [0x90, 0x90, 0xE8, 1, 2, 3, 4] } else { *b"DINGPAD" })
    }

    #[test]
    fn attributes_leaf_function_hotspot_and_verified_caller() {
        let m = map();
        let mut a = Aggregate::default();
        a.add(Phase::Playing, 0x0050_0010, &[], &m, code);
        a.add(Phase::Playing, 0x0050_0030, &[], &m, code); // same 0x40 bucket
        // In ntdll!NtCreateFile; 0x0060_0000 is a stale value (no call
        // before it) and must be skipped in favour of 0x0070_0000.
        a.add(Phase::Loading, 0x7700_1234, &[0x1234, 0x6EB3_1000, 0x0060_0000, 0x0070_0000], &m, code);
        a.add(Phase::Loading, 0x0000_0010, &[], &m, code);

        assert_eq!(a.samples(), 4);
        assert_eq!(a.by_module[&(Phase::Playing, "Em4.exe".to_string())], 2);
        assert_eq!(a.hotspots[&(Phase::Playing, "Em4.exe".to_string(), 0x10_0000)], 2);
        assert_eq!(a.by_function[&(Phase::Loading, "ntdll.dll!NtCreateFile".to_string())], 1);
        assert_eq!(
            a.callers[&(Phase::Loading, "ntdll.dll!NtCreateFile".to_string(), "Em4.exe+0x300000".to_string())],
            1
        );
        assert_eq!(a.by_module[&(Phase::Loading, "(unbekannt)".to_string())], 1);

        let mut s = String::new();
        a.render_phase(Phase::Playing, &mut s);
        assert!(s.contains("100.0 %  Em4.exe"), "{s}");
        a.render_phase(Phase::Loading, &mut s);
        assert!(s.contains("ntdll.dll!NtCreateFile"), "{s}");
    }

    #[test]
    fn no_verified_caller() {
        let m = map();
        let mut a = Aggregate::default();
        a.add(Phase::Playing, 0x7700_2004, &[0x0060_0000], &m, code);
        assert_eq!(
            a.callers[&(Phase::Playing, "ntdll.dll!NtClose".to_string(), "(kein Spielcode gefunden)".to_string())],
            1
        );
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe("Em4.exe"), "Spiellogik, Skripte, KI, Simulation");
        assert_eq!(describe("nvoglv32.dll"), "Grafiktreiber (OpenGL)");
        assert_eq!(describe("opengl32.dll (ReShade)"), "ReShade (Post-Processing)");
    }
}
