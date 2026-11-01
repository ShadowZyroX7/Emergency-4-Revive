//! Data for the "Statistik" tab: last session (stats CSV + mod log) and
//! crash reports (minidumps written by EM4 Revive).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub fps: f64,
    pub low: f64,
    pub ram_mb: f64,
    /// Used 32-bit address space (0 in files from older versions).
    pub addr_mb: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    pub samples: Vec<Sample>,
    pub when: Option<SystemTime>,
    pub map_load_s: Option<u32>,
}

/// Rows below this FPS are loading screens.
pub const LOADING_FPS: f64 = 5.0;
/// Seconds after a loading screen that are not judged as stutter.
pub const SETTLE_S: f64 = 3.0;

impl Session {
    pub fn playing(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter().filter(|s| s.fps >= LOADING_FPS)
    }
    pub fn duration_s(&self) -> f64 {
        self.samples.last().map(|s| s.t).unwrap_or(0.0)
    }
    pub fn avg_fps(&self) -> Option<f64> {
        avg(self.playing().map(|s| s.fps))
    }
    /// Playing samples, skipping the first seconds after each loading screen
    /// (the transition frames are not representative stutters).
    pub fn steady(&self) -> Vec<&Sample> {
        let mut last_loading = f64::NEG_INFINITY;
        let mut out = Vec::new();
        for s in &self.samples {
            if s.fps < LOADING_FPS {
                last_loading = s.t;
            } else if s.t - last_loading > SETTLE_S {
                out.push(s);
            }
        }
        out
    }
    pub fn avg_low(&self) -> Option<f64> {
        avg(self.steady().into_iter().map(|s| s.low))
    }
    pub fn worst_low(&self) -> Option<f64> {
        self.steady().into_iter().map(|s| s.low).fold(None, |m, v| Some(m.map_or(v, |m: f64| m.min(v))))
    }
    pub fn ram_peak(&self) -> Option<f64> {
        self.samples.iter().map(|s| s.ram_mb).fold(None, |m, v| Some(m.map_or(v, |m: f64| m.max(v))))
    }
    /// Peak used address space; None for files without that column.
    pub fn address_peak(&self) -> Option<f64> {
        self.samples.iter().map(|s| s.addr_mb).filter(|&v| v > 0.0).fold(None, |m, v| Some(m.map_or(v, |m: f64| m.max(v))))
    }
}

fn avg(it: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = it.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

/// Parse `em4revive_stats.csv` (German number format, `;` separated).
pub fn parse_csv(text: &str) -> Vec<Sample> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<f64> = l.split(';').map(|v| v.trim().replace(',', ".").parse().unwrap_or(f64::NAN)).collect();
            (c.len() >= 6 && c[0].is_finite() && c[1].is_finite()).then(|| Sample {
                t: c[0],
                fps: c[1],
                low: if c[2].is_finite() { c[2] } else { 0.0 },
                ram_mb: if c[5].is_finite() { c[5] } else { 0.0 },
                addr_mb: c.get(8).copied().filter(|v| v.is_finite()).unwrap_or(0.0),
            })
        })
        .collect()
}

/// "Direktstart abgeschlossen: Karte in 39 s geladen" -> 39
pub fn parse_map_load(log: &str) -> Option<u32> {
    log.lines().rev().find_map(|l| {
        let i = l.find("Karte in ")? + "Karte in ".len();
        let rest = &l[i..];
        let end = rest.find(' ')?;
        rest[..end].parse().ok()
    })
}

pub fn load_session(game_dir: &Path) -> Session {
    let csv = game_dir.join("em4revive_stats.csv");
    Session {
        samples: std::fs::read_to_string(&csv).map(|t| parse_csv(&t)).unwrap_or_default(),
        when: std::fs::metadata(&csv).and_then(|m| m.modified()).ok(),
        map_load_s: std::fs::read_to_string(game_dir.join("em4revive.log")).ok().and_then(|t| parse_map_load(&t)),
    }
}

// ---- crash reports ----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CrashInfo {
    pub code: u32,
    pub module: String,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct CrashReport {
    pub path: PathBuf,
    pub when: Option<SystemTime>,
    pub size: u64,
    pub info: Option<CrashInfo>,
}

impl CrashInfo {
    pub fn location(&self) -> String {
        format!("{}+0x{:X}", self.module, self.offset)
    }

    /// Human assessment of known crash sites.
    pub fn assessment(&self) -> &'static str {
        match (self.module.to_ascii_lowercase().as_str(), self.offset) {
            ("em4.exe", 0x4C1199) => "Bekannter Absturz beim Beenden (Spiel/Mod) – harmlos",
            (m, _) if m.contains("dinput8") => "In EM4 Revive – bitte melden!",
            ("em4.exe", _) => "Im Spielcode (oft Mod-Skripte)",
            (m, _) if m.starts_with("nvoglv") || m.starts_with("atio") || m.starts_with("ig") => "Im Grafiktreiber",
            _ => "Unbekannt",
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Exception code and faulting module/offset from a minidump
/// (exception stream #6 and module list #4).
pub fn parse_minidump(b: &[u8]) -> Option<CrashInfo> {
    if b.get(0..4)? != b"MDMP" {
        return None;
    }
    let n = u32_at(b, 8)? as usize;
    let dir = u32_at(b, 12)? as usize;
    let (mut exc, mut mods) = (None, None);
    for i in 0..n.min(64) {
        let e = dir + i * 12;
        match u32_at(b, e)? {
            6 => exc = Some(u32_at(b, e + 8)? as usize),
            4 => mods = Some(u32_at(b, e + 8)? as usize),
            _ => {}
        }
    }
    let exc = exc?;
    let code = u32_at(b, exc + 8)?;
    let addr = u64_at(b, exc + 24)?;
    let mut module = "(unbekannt)".to_string();
    let mut offset = addr;
    if let Some(ml) = mods {
        let count = u32_at(b, ml)? as usize;
        for i in 0..count.min(4096) {
            let m = ml + 4 + i * 108;
            let base = u64_at(b, m)?;
            let size = u32_at(b, m + 8)? as u64;
            if addr >= base && addr < base + size {
                let name_rva = u32_at(b, m + 20)? as usize;
                let len = u32_at(b, name_rva)? as usize;
                let units: Vec<u16> =
                    (0..len / 2).filter_map(|k| u16_at(b, name_rva + 4 + k * 2)).collect();
                let full = String::from_utf16_lossy(&units);
                module = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
                offset = addr - base;
                break;
            }
        }
    }
    Some(CrashInfo { code, module, offset })
}

pub fn crash_reports(game_dir: &Path) -> Vec<CrashReport> {
    let Ok(entries) = std::fs::read_dir(game_dir) else { return Vec::new() };
    let mut v: Vec<CrashReport> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            n.starts_with("em4revive_crash_") && n.ends_with(".dmp")
        })
        .map(|e| {
            let meta = e.metadata().ok();
            // Only the header region is needed; dumps are ~24 MB.
            let info = std::fs::read(e.path()).ok().and_then(|b| parse_minidump(&b));
            CrashReport {
                path: e.path(),
                when: meta.as_ref().and_then(|m| m.modified().ok()),
                size: meta.map(|m| m.len()).unwrap_or(0),
                info,
            }
        })
        .collect();
    v.sort_by(|a, b| b.when.cmp(&a.when));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "zeit_s;fps_avg;fps_1pct_low;frametime_max_ms;draw_calls_avg;ram_privat_mb;ram_working_set_mb;bildwechsel_wartezeit_ms\n\
        4,4;0,6;0,6;1635,84;0;114;119;0,04\n\
        65,8;100,0;50,0;6,89;88;452;465;0,03\n\
        66,8;60,0;30,0;3,61;88;900;465;0,18\n";

    #[test]
    fn address_space_column() {
        assert_eq!(Session { samples: parse_csv(CSV), when: None, map_load_s: None }.address_peak(), None);
        let csv = "h\n1,0;60,0;40,0;1;0;800;700;0;1500\n2,0;60,0;40,0;1;0;800;700;0;1730\n";
        assert_eq!(Session { samples: parse_csv(csv), when: None, map_load_s: None }.address_peak(), Some(1730.0));
    }

    #[test]
    fn session_numbers() {
        let s = Session { samples: parse_csv(CSV), when: None, map_load_s: None };
        assert_eq!(s.samples.len(), 3);
        assert_eq!(s.avg_fps(), Some(80.0));
        // Row at 65,8 s follows the loading row at 4,4 s by > 3 s -> counted.
        assert_eq!(s.avg_low(), Some(40.0));
        assert_eq!(s.worst_low(), Some(30.0));
        // A row right after a loading screen is not judged.
        let csv2 = "h\n10,0;0,5;0,5;1;0;1;1;0\n11,0;20,0;1,0;1;0;1;1;0\n20,0;60,0;40,0;1;0;1;1;0\n";
        let s2 = Session { samples: parse_csv(csv2), when: None, map_load_s: None };
        assert_eq!(s2.worst_low(), Some(40.0));
        assert_eq!(s.ram_peak(), Some(900.0));
        assert!((s.duration_s() - 66.8).abs() < 1e-9);
        assert_eq!(Session::default().avg_fps(), None);
    }

    #[test]
    fn map_load_from_log() {
        let log = "x\n19:00:26.462 [ 7692] INFO  | Direktstart abgeschlossen: Karte in 39 s geladen\ny";
        assert_eq!(parse_map_load(log), Some(39));
        assert_eq!(parse_map_load("nichts"), None);
    }

    #[test]
    fn assessments() {
        let exit = CrashInfo { code: 0xC0000005, module: "Em4.exe".into(), offset: 0x4C1199 };
        assert!(exit.assessment().contains("Beenden"));
        assert_eq!(exit.location(), "Em4.exe+0x4C1199");
        let ours = CrashInfo { code: 0xC0000005, module: "dinput8.dll".into(), offset: 0x10 };
        assert!(ours.assessment().contains("EM4 Revive"));
    }

    #[test]
    fn garbage_dump_is_none() {
        assert!(parse_minidump(b"nope").is_none());
        assert!(parse_minidump(b"MDMP\0\0\0\0").is_none());
    }

    /// Real dumps from the installation, if present.
    #[test]
    fn parses_real_dumps() {
        let game = Path::new(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe");
        let reports = crash_reports(game);
        for r in &reports {
            let info = r.info.as_ref().unwrap_or_else(|| panic!("{} nicht lesbar", r.path.display()));
            assert_eq!(info.code, 0xC0000005);
            assert!(info.module.eq_ignore_ascii_case("Em4.exe"), "{}", info.location());
        }
        // The known exit crash must be recognised in at least one of them.
        if !reports.is_empty() {
            assert!(reports.iter().any(|r| r.info.as_ref().is_some_and(|i| i.offset == 0x4C1199)));
        }
    }
}
