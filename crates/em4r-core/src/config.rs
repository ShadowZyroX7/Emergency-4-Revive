//! `em4revive.toml` – loaded once at startup.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;

/// The commented default file that is written when none exists.
pub const DEFAULT_TOML: &str = include_str!("../../../assets/em4revive.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub perf: Perf,
    pub stats: Stats,
    pub gl: Gl,
    pub crash: Crash,
    pub profiler: Profiler,
    pub game: Game,
    pub fx: Fx,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fx {
    pub enabled: bool,
    pub capture: bool,
    pub sharpen: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub warmth: f32,
    pub filmic: f32,
    pub ssao: bool,
    pub ssao_strength: f32,
    pub ssao_radius: f32,
    pub dof: bool,
    pub dof_strength: f32,
    pub dof_start: f32,
    pub dof_end: f32,
}

impl Default for Fx {
    fn default() -> Self {
        Self {
            enabled: false,
            capture: false,
            sharpen: 0.5,
            contrast: 1.06,
            saturation: 1.10,
            vibrance: 0.15,
            warmth: 0.10,
            filmic: 0.25,
            ssao: true,
            ssao_strength: 1.0,
            ssao_radius: 60.0,
            dof: true,
            dof_strength: 0.5,
            dof_start: 9000.0,
            dof_end: 22000.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Game {
    pub skip_intro: bool,
    pub fast_logfile: bool,
    pub fast_exit: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self { skip_intro: true, fast_logfile: true, fast_exit: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profiler {
    pub enabled: bool,
    pub sample_hz: u32,
    pub file_io: bool,
    pub report_interval_sec: u32,
    pub frame_trace: bool,
}

impl Default for Profiler {
    fn default() -> Self {
        Self { enabled: false, sample_hz: 1000, file_io: true, report_interval_sec: 15, frame_trace: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub enabled: bool,
    pub debug_log: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Perf {
    pub high_res_timer: bool,
    pub frame_limit: u32,
    pub vsync: i32,
    pub dpi_aware: bool,
    pub missing_file_cache: bool,
    pub fast_zlib: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stats {
    pub enabled: bool,
    pub csv: bool,
    pub interval_sec: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gl {
    pub force_depth_bits: u8,
    pub force_stencil_bits: u8,
    pub msaa: u32,
    pub anisotropy: u32,
    pub force_trilinear: bool,
    pub count_draw_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Crash {
    pub minidump: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            perf: Perf::default(),
            stats: Stats::default(),
            gl: Gl::default(),
            crash: Crash::default(),
            profiler: Profiler::default(),
            game: Game::default(),
            fx: Fx::default(),
        }
    }
}
impl Default for General {
    fn default() -> Self {
        Self { enabled: true, debug_log: false }
    }
}
impl Default for Perf {
    fn default() -> Self {
        Self { high_res_timer: true, frame_limit: 0, vsync: -1, dpi_aware: true, missing_file_cache: true, fast_zlib: true }
    }
}
impl Default for Stats {
    fn default() -> Self {
        Self { enabled: true, csv: true, interval_sec: 1.0 }
    }
}
impl Default for Gl {
    fn default() -> Self {
        Self {
            force_depth_bits: 24,
            force_stencil_bits: 8,
            msaa: 4,
            anisotropy: 16,
            force_trilinear: true,
            count_draw_calls: true,
        }
    }
}
impl Default for Crash {
    fn default() -> Self {
        Self { minidump: true }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        // Clamped values are still usable; report them once the log is open.
        for note in cfg.sanitize() {
            crate::log::defer_warning(format!("Config: {note}"));
        }
        Ok(cfg)
    }

    /// Clamp out-of-range values to safe ones. Returns human-readable notes.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let g = &mut self.gl;
        if !matches!(g.force_depth_bits, 0 | 16 | 24 | 32) {
            notes.push(format!("gl.force_depth_bits={} ungültig -> 24", g.force_depth_bits));
            g.force_depth_bits = 24;
        }
        if g.force_stencil_bits > 8 {
            notes.push(format!("gl.force_stencil_bits={} -> 8", g.force_stencil_bits));
            g.force_stencil_bits = 8;
        }
        if !matches!(g.msaa, 0 | 2 | 4 | 8) {
            notes.push(format!("gl.msaa={} ungültig -> 4", g.msaa));
            g.msaa = 4;
        }
        if g.anisotropy > 16 {
            notes.push(format!("gl.anisotropy={} -> 16", g.anisotropy));
            g.anisotropy = 16;
        }
        let p = &mut self.perf;
        if p.frame_limit > 1000 {
            notes.push(format!("perf.frame_limit={} -> 1000", p.frame_limit));
            p.frame_limit = 1000;
        }
        if !(-1..=4).contains(&p.vsync) {
            notes.push(format!("perf.vsync={} ungültig -> -1", p.vsync));
            p.vsync = -1;
        }
        let fx = &mut self.fx;
        let clamp = |v: &mut f32, lo: f32, hi: f32, def: f32, name: &str, notes: &mut Vec<String>| {
            if !(v.is_finite() && *v >= lo && *v <= hi) {
                notes.push(format!("fx.{name}={v} -> {def}"));
                *v = def;
            }
        };
        let d = Fx::default();
        clamp(&mut fx.sharpen, 0.0, 1.0, d.sharpen, "sharpen", &mut notes);
        clamp(&mut fx.contrast, 0.5, 1.5, d.contrast, "contrast", &mut notes);
        clamp(&mut fx.saturation, 0.0, 2.0, d.saturation, "saturation", &mut notes);
        clamp(&mut fx.vibrance, -1.0, 1.0, d.vibrance, "vibrance", &mut notes);
        clamp(&mut fx.warmth, -1.0, 1.0, d.warmth, "warmth", &mut notes);
        clamp(&mut fx.filmic, 0.0, 1.0, d.filmic, "filmic", &mut notes);
        clamp(&mut fx.ssao_strength, 0.0, 2.0, d.ssao_strength, "ssao_strength", &mut notes);
        clamp(&mut fx.ssao_radius, 5.0, 500.0, d.ssao_radius, "ssao_radius", &mut notes);
        clamp(&mut fx.dof_strength, 0.0, 1.0, d.dof_strength, "dof_strength", &mut notes);
        clamp(&mut fx.dof_start, 100.0, 32000.0, d.dof_start, "dof_start", &mut notes);
        clamp(&mut fx.dof_end, 100.0, 32000.0, d.dof_end, "dof_end", &mut notes);
        let pr = &mut self.profiler;
        if !(100..=2000).contains(&pr.sample_hz) {
            notes.push(format!("profiler.sample_hz={} -> 1000", pr.sample_hz));
            pr.sample_hz = 1000;
        }
        if !(5..=600).contains(&pr.report_interval_sec) {
            notes.push(format!("profiler.report_interval_sec={} -> 15", pr.report_interval_sec));
            pr.report_interval_sec = 15;
        }
        let s = &mut self.stats;
        if !(s.interval_sec.is_finite() && (0.1..=60.0).contains(&s.interval_sec)) {
            notes.push(format!("stats.interval_sec={} -> 1.0", s.interval_sec));
            s.interval_sec = 1.0;
        }
        notes
    }

    /// Load the config; write the default file when missing.
    /// Never fails: on errors the defaults are used and the error is returned.
    pub fn load_or_create(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Config::parse(text.trim_start_matches('\u{feff}')) {
                Ok(cfg) => (cfg, None),
                Err(e) => (
                    Config::default(),
                    Some(format!("{} fehlerhaft, verwende Standardwerte: {e}", path.display())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let err = std::fs::write(path, DEFAULT_TOML)
                    .err()
                    .map(|e| format!("Konnte {} nicht anlegen: {e}", path.display()));
                (Config::default(), err)
            }
            Err(e) => (Config::default(), Some(format!("Konnte {} nicht lesen: {e}", path.display()))),
        }
    }
}

static GLOBAL: OnceLock<Config> = OnceLock::new();

/// Install the process-wide config (first call wins).
pub fn set_global(cfg: Config) {
    let _ = GLOBAL.set(cfg);
}

/// Process-wide config; defaults if not yet set.
pub fn global() -> &'static Config {
    GLOBAL.get_or_init(Config::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_default_file_matches_code_defaults() {
        assert_eq!(Config::parse(DEFAULT_TOML).unwrap(), Config::default());
    }

    #[test]
    fn partial_file_falls_back_to_defaults() {
        let cfg = Config::parse("[perf]\nframe_limit = 60\n").unwrap();
        assert_eq!(cfg.perf.frame_limit, 60);
        assert_eq!(cfg.gl, Gl::default());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("[gl]\nanisotropie = 4\n").is_err());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let cfg = Config::parse("[gl]\nforce_depth_bits = 20\nanisotropy = 64\n[perf]\nvsync = 9\n").unwrap();
        assert_eq!(cfg.gl.force_depth_bits, 24);
        assert_eq!(cfg.gl.anisotropy, 16);
        assert_eq!(cfg.perf.vsync, -1);
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = std::env::temp_dir().join(format!("em4r_cfg_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("em4revive.toml");
        let _ = std::fs::remove_file(&path);
        let (cfg, err) = Config::load_or_create(&path);
        assert!(err.is_none());
        assert_eq!(cfg, Config::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_TOML);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_file_yields_defaults_and_error() {
        let dir = std::env::temp_dir().join(format!("em4r_cfg_bad_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("em4revive.toml");
        std::fs::write(&path, "[gl\nbroken").unwrap();
        let (cfg, err) = Config::load_or_create(&path);
        assert!(err.is_some());
        assert_eq!(cfg, Config::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
