//! Editing `em4revive.toml` from the launcher while keeping all comments.

use em4r_core::Config;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item};

pub struct ReviveConfig {
    path: PathBuf,
    doc: DocumentMut,
    /// Parsed view (with defaults for missing keys) – what the mod will use.
    pub cfg: Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Leistung,
    Ausgewogen,
    Qualitaet,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Leistung, Preset::Ausgewogen, Preset::Qualitaet];
    pub fn label(self) -> &'static str {
        match self {
            Preset::Leistung => "Leistung",
            Preset::Ausgewogen => "Ausgewogen",
            Preset::Qualitaet => "Qualität",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Preset::Leistung => "Maximale FPS: keine Kantenglättung, 4x Texturfilter, kein FPS-Limit",
            Preset::Ausgewogen => "Empfohlen: 4x MSAA, 16x Texturfilter, 24-Bit-Tiefe, FPS auf 144 begrenzt",
            Preset::Qualitaet => "Beste Optik: 8x MSAA, 16x Texturfilter, Trilinear, VSync an",
        }
    }
}

impl ReviveConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => em4r_core::config::DEFAULT_TOML.to_string(),
            Err(e) => return Err(e.to_string()),
        };
        let text = text.trim_start_matches('\u{feff}');
        let doc: DocumentMut = text.parse().map_err(|e| format!("em4revive.toml fehlerhaft: {e}"))?;
        let cfg = Config::parse(text).map_err(|e| format!("em4revive.toml fehlerhaft: {e}"))?;
        Ok(Self { path: path.to_path_buf(), doc, cfg })
    }

    /// Write `self.cfg` back into the document (only values change).
    pub fn save(&mut self) -> Result<(), String> {
        let c = self.cfg.clone();
        self.set("general", "enabled", value(c.general.enabled));
        self.set("game", "skip_intro", value(c.game.skip_intro));
        self.set("game", "fast_logfile", value(c.game.fast_logfile));
        self.set("game", "fast_exit", value(c.game.fast_exit));
        self.set("window", "borderless", value(c.window.borderless));
        self.set("window", "pause_in_background", value(c.window.pause_in_background));
        self.set("window", "allow_system_keys", value(c.window.allow_system_keys));
        self.set("perf", "frame_limit", value(c.perf.frame_limit as i64));
        self.set("perf", "vsync", value(c.perf.vsync as i64));
        self.set("perf", "missing_file_cache", value(c.perf.missing_file_cache));
        self.set("perf", "fast_zlib", value(c.perf.fast_zlib));
        self.set("perf", "high_res_timer", value(c.perf.high_res_timer));
        self.set("stats", "enabled", value(c.stats.enabled));
        self.set("gl", "force_depth_bits", value(c.gl.force_depth_bits as i64));
        self.set("gl", "force_stencil_bits", value(c.gl.force_stencil_bits as i64));
        self.set("gl", "anisotropy", value(c.gl.anisotropy as i64));
        self.set("gl", "msaa", value(c.gl.msaa as i64));
        self.set("gl", "force_trilinear", value(c.gl.force_trilinear));
        self.set("crash", "minidump", value(c.crash.minidump));
        self.set("profiler", "enabled", value(c.profiler.enabled));
        let r2 = |v: f32| ((v as f64) * 100.0).round() / 100.0;
        self.set("fx", "enabled", value(c.fx.enabled));
        self.set("fx", "sharpen", value(r2(c.fx.sharpen)));
        self.set("fx", "contrast", value(r2(c.fx.contrast)));
        self.set("fx", "saturation", value(r2(c.fx.saturation)));
        self.set("fx", "vibrance", value(r2(c.fx.vibrance)));
        self.set("fx", "warmth", value(r2(c.fx.warmth)));
        self.set("fx", "filmic", value(r2(c.fx.filmic)));
        self.set("fx", "ssao", value(c.fx.ssao));
        self.set("fx", "ssao_strength", value(r2(c.fx.ssao_strength)));
        self.set("fx", "ssao_radius", value(r2(c.fx.ssao_radius)));
        self.set("fx", "dof", value(c.fx.dof));
        self.set("fx", "dof_strength", value(r2(c.fx.dof_strength)));
        self.set("fx", "dof_start", value(r2(c.fx.dof_start)));
        self.set("fx", "dof_end", value(r2(c.fx.dof_end)));
        let text = self.doc.to_string();
        // Never write something the mod could not read back.
        Config::parse(&text).map_err(|e| format!("interner Fehler, nicht gespeichert: {e}"))?;
        std::fs::write(&self.path, text).map_err(|e| e.to_string())
    }

    /// Set `[table] key = value`, keeping the key's comment and position.
    fn set(&mut self, table: &str, key: &str, v: Item) {
        if !self.doc.contains_key(table) {
            self.doc[table] = toml_edit::table();
        }
        let t = &mut self.doc[table];
        match t.get_mut(key) {
            Some(existing) => {
                // Keep the decoration (comments) of the existing value.
                if let (Some(old), Item::Value(mut new)) = (existing.as_value(), v.clone()) {
                    *new.decor_mut() = old.decor().clone();
                    *existing = Item::Value(new);
                } else {
                    *existing = v;
                }
            }
            None => t[key] = v,
        }
    }

    pub fn apply_preset(&mut self, p: Preset) {
        let c = &mut self.cfg;
        c.gl.force_depth_bits = 24;
        c.gl.force_stencil_bits = 8;
        match p {
            Preset::Leistung => {
                c.gl.msaa = 0;
                c.gl.anisotropy = 4;
                c.gl.force_trilinear = false;
                c.perf.frame_limit = 0;
                c.perf.vsync = 0;
            }
            Preset::Ausgewogen => {
                c.gl.msaa = 4;
                c.gl.anisotropy = 16;
                c.gl.force_trilinear = true;
                c.perf.frame_limit = 144;
                c.perf.vsync = -1;
            }
            Preset::Qualitaet => {
                c.gl.msaa = 8;
                c.gl.anisotropy = 16;
                c.gl.force_trilinear = true;
                c.perf.frame_limit = 0;
                c.perf.vsync = 1;
            }
        }
        c.perf.missing_file_cache = true;
        c.game.fast_logfile = true;
        let d = em4r_core::config::Fx::default();
        match p {
            Preset::Leistung => c.fx.enabled = false,
            Preset::Ausgewogen => c.fx = em4r_core::config::Fx { enabled: true, capture: c.fx.capture, ..d },
            Preset::Qualitaet => {
                c.fx = em4r_core::config::Fx { enabled: true, capture: c.fx.capture, sharpen: 0.65, ..d }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("em4r_rcfg_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("em4revive.toml")
    }

    #[test]
    fn edits_values_and_keeps_comments() {
        let p = tmp("comments");
        std::fs::write(&p, em4r_core::config::DEFAULT_TOML).unwrap();
        let mut rc = ReviveConfig::load(&p).unwrap();
        rc.cfg.perf.frame_limit = 120;
        rc.cfg.gl.anisotropy = 8;
        rc.cfg.game.skip_intro = false;
        rc.save().unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("frame_limit = 120"), "{text}");
        assert!(text.contains("anisotropy = 8"));
        assert!(text.contains("skip_intro = false"));
        // The explanatory comments survive.
        assert!(text.contains("# FPS-Limit. 0 = aus."));
        assert!(text.contains("# Sixteen-Tons-Logo"));
        let back = Config::parse(&text).unwrap();
        assert_eq!(back.perf.frame_limit, 120);
        assert_eq!(back.gl.anisotropy, 8);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn adds_missing_sections() {
        let p = tmp("missing");
        std::fs::write(&p, "[general]\nenabled = true\n").unwrap();
        let mut rc = ReviveConfig::load(&p).unwrap();
        rc.cfg.game.fast_exit = false;
        rc.save().unwrap();
        let back = Config::parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(!back.game.fast_exit);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn presets() {
        let p = tmp("preset");
        let mut rc = ReviveConfig::load(&p).unwrap(); // file missing -> defaults
        rc.apply_preset(Preset::Leistung);
        assert_eq!(rc.cfg.gl.anisotropy, 4);
        rc.apply_preset(Preset::Qualitaet);
        assert_eq!(rc.cfg.perf.vsync, 1);
        assert!(rc.cfg.fx.enabled);
        rc.cfg.fx.contrast = 1.234;
        rc.save().unwrap();
        assert!(p.exists());
        // fx values survive a save/load round trip (rounded to 2 decimals).
        let back = ReviveConfig::load(&p).unwrap();
        assert!(back.cfg.fx.enabled);
        assert!((back.cfg.fx.contrast - 1.23).abs() < 1e-6);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
