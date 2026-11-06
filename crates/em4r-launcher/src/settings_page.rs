//! The "Einstellungen" tab: EM4 Revive options and selected game options.

use crate::em4cfg::{self, Em4Cfg};
use crate::revive_cfg::{Preset, ReviveConfig};
use eframe::egui::{self, Color32, RichText};
use em4r_core::Config;
use std::path::{Path, PathBuf};

const MUTED: Color32 = Color32::from_rgb(150, 154, 160);
const OK: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const CARD: Color32 = Color32::from_rgb(32, 34, 38);

pub struct SettingsPage {
    game_dir: PathBuf,
    revive: Result<ReviveConfig, String>,
    em4: Result<Em4Cfg, String>,
    modes: Vec<(u32, u32)>,
    // Pending game options.
    resolution: (u32, u32),
    fullscreen: bool,
    bloom: bool,
    shadows: bool,
    /// State of both files as last read or saved, to detect unsaved changes.
    saved_revive: Option<Config>,
    saved_game: GameOptions,
    message: Option<(bool, String)>,
}

/// Resolution, fullscreen, bloom, shadows.
type GameOptions = ((u32, u32), bool, bool, bool);

impl SettingsPage {
    pub fn new(game_dir: &Path) -> Self {
        let mut p = Self {
            game_dir: game_dir.to_path_buf(),
            revive: Err(String::new()),
            em4: Err(String::new()),
            modes: em4cfg::display_modes(),
            resolution: (1920, 1080),
            fullscreen: true,
            bloom: true,
            shadows: true,
            saved_revive: None,
            saved_game: ((1920, 1080), true, true, true),
            message: None,
        };
        p.reload();
        p
    }

    /// (Re)read both files, discarding unsaved changes.
    pub fn reload(&mut self) {
        self.revive = ReviveConfig::load(&self.game_dir.join("em4revive.toml"));
        self.em4 = Em4Cfg::load(&self.game_dir);
        if let Ok(c) = &self.em4 {
            let num = |n: &str| c.get(n).and_then(|v| v.trim().parse::<u32>().ok());
            if let (Some(x), Some(y)) = (num("r_xres"), num("r_yres")) {
                self.resolution = (x, y);
            }
            self.fullscreen = c.get_bool("r_fullscreen");
            self.bloom = c.get_bool("e4_bloom");
            self.shadows = c.get_bool("e4_shadow");
        }
        if !self.modes.contains(&self.resolution) {
            self.modes.push(self.resolution);
            self.modes.sort();
        }
        self.saved_revive = self.revive.as_ref().ok().map(|r| r.cfg.clone());
        self.saved_game = self.game_options();
    }

    fn game_options(&self) -> GameOptions {
        (self.resolution, self.fullscreen, self.bloom, self.shadows)
    }

    /// Does the page hold changes that are not in the files yet?
    pub fn has_unsaved_changes(&self) -> bool {
        let revive = matches!(&self.revive, Ok(r) if self.saved_revive.as_ref() != Some(&r.cfg));
        let game = self.em4.is_ok() && self.game_options() != self.saved_game;
        revive || game
    }

    /// Returns whether everything was written.
    pub fn save(&mut self) -> bool {
        let mut notes = Vec::new();
        let mut ok = true;
        if let Ok(r) = self.revive.as_mut() {
            match r.save() {
                Ok(()) => {
                    self.saved_revive = Some(r.cfg.clone());
                    notes.push("EM4-Revive-Einstellungen gespeichert".to_string());
                }
                Err(e) => {
                    ok = false;
                    notes.push(format!("em4revive.toml: {e}"));
                }
            }
        }
        let game_options = self.game_options();
        if let Ok(c) = self.em4.as_mut() {
            let b = |v: bool| if v { "1".to_string() } else { "0".to_string() };
            let changes = [
                ("r_xres", self.resolution.0.to_string()),
                ("r_yres", self.resolution.1.to_string()),
                ("r_fullscreen", b(self.fullscreen)),
                ("e4_bloom", b(self.bloom)),
                ("e4_shadow", b(self.shadows)),
            ];
            match c.save(&changes, &self.game_dir.join("_em4revive_backup")) {
                Ok(n) => {
                    self.saved_game = game_options;
                    if n > 0 {
                        notes.push(format!("{n} Spieleinstellung(en) in em4.cfg gespeichert"));
                    }
                }
                Err(e) => {
                    ok = false;
                    notes.push(format!("em4.cfg: {e}"));
                }
            }
        }
        self.message = Some((ok, notes.join(" · ")));
        ok
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_running: bool) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            if game_running {
                ui.colored_label(WARN, "Das Spiel läuft – Änderungen bitte erst nach dem Beenden speichern.");
                ui.add_space(6.0);
            }
            ui.add_enabled_ui(!game_running, |ui| {
                self.revive_section(ui);
                ui.add_space(14.0);
                self.game_section(ui);
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let save = egui::Button::new(RichText::new("Speichern").strong().color(Color32::WHITE))
                        .fill(Color32::from_rgb(214, 48, 49))
                        .min_size(egui::vec2(140.0, 36.0));
                    if ui.add(save).clicked() {
                        self.save();
                    }
                    if ui.add(egui::Button::new("Verwerfen").min_size(egui::vec2(110.0, 36.0))).clicked() {
                        self.reload();
                        self.message = Some((true, "Ungespeicherte Änderungen verworfen".into()));
                    }
                });
            });
            if let Some((ok, m)) = &self.message {
                ui.add_space(6.0);
                ui.colored_label(if *ok { OK } else { WARN }, m);
            }
            ui.add_space(20.0);
        });
    }

    fn revive_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "EM4 Revive", |ui| {
            let r = match self.revive.as_mut() {
                Ok(r) => r,
                Err(e) => {
                    ui.colored_label(WARN, e.as_str());
                    return;
                }
            };
            ui.checkbox(&mut r.cfg.general.enabled, RichText::new("EM4 Revive aktiv").strong())
                .on_hover_text("Hauptschalter. Aus = das Spiel läuft wie im Original.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Voreinstellung:");
                for p in Preset::ALL {
                    if ui.button(p.label()).on_hover_text(p.description()).clicked() {
                        r.apply_preset(p);
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Grafik").strong());
            let msaa_label = |v: u32| if v == 0 { "aus".to_string() } else { format!("{v}x MSAA") };
            egui::ComboBox::from_label("Kantenglättung")
                .selected_text(msaa_label(r.cfg.gl.msaa))
                .show_ui(ui, |ui| {
                    for v in [0u32, 2, 4, 8] {
                        ui.selectable_value(&mut r.cfg.gl.msaa, v, msaa_label(v));
                    }
                })
                .response
                .on_hover_text("Glättet treppenartige Kanten. 4x ist ein guter Kompromiss.");
            let af_label = |v: u32| if v == 0 { "Spiel entscheidet".to_string() } else { format!("{v}x") };
            egui::ComboBox::from_label("Texturfilter (anisotrop)")
                .selected_text(af_label(r.cfg.gl.anisotropy))
                .show_ui(ui, |ui| {
                    for v in [0u32, 2, 4, 8, 16] {
                        ui.selectable_value(&mut r.cfg.gl.anisotropy, v, af_label(v));
                    }
                });
            ui.checkbox(&mut r.cfg.gl.force_trilinear, "Weichere Übergänge zwischen Textur-Stufen (trilinear)");
            let mut depth = r.cfg.gl.force_depth_bits >= 24;
            if ui
                .checkbox(&mut depth, "24-Bit-Tiefenpuffer (weniger Flackern auf Distanz)")
                .on_hover_text("Das Spiel fordert nur 16 Bit an; das verursacht flackernde Flächen in der Ferne.")
                .changed()
            {
                r.cfg.gl.force_depth_bits = if depth { 24 } else { 0 };
                r.cfg.gl.force_stencil_bits = if depth { 8 } else { 0 };
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Effekte (nur 3D-Welt, Menüs bleiben unverändert)").strong());
            ui.checkbox(&mut r.cfg.fx.enabled, "Eigene Grafik-Effekte aktiv");
            ui.add_enabled_ui(r.cfg.fx.enabled, |ui| {
                let fx = &mut r.cfg.fx;
                ui.add(egui::Slider::new(&mut fx.sharpen, 0.0..=1.0).text("Nachschärfen"));
                ui.add(egui::Slider::new(&mut fx.contrast, 0.8..=1.3).text("Kontrast"));
                ui.add(egui::Slider::new(&mut fx.saturation, 0.5..=1.6).text("Sättigung"));
                ui.add(egui::Slider::new(&mut fx.vibrance, -0.5..=0.8).text("Lebendigkeit"));
                ui.add(egui::Slider::new(&mut fx.warmth, -1.0..=1.0).text("Farbtemperatur (− kühl / + warm)"));
                ui.add(egui::Slider::new(&mut fx.filmic, 0.0..=1.0).text("Filmische Kurve"));
                ui.checkbox(&mut fx.ssao, "Umgebungsverdeckung (SSAO)");
                ui.add_enabled_ui(fx.ssao, |ui| {
                    ui.add(egui::Slider::new(&mut fx.ssao_strength, 0.0..=2.0).text("SSAO-Stärke"));
                    ui.add(egui::Slider::new(&mut fx.ssao_radius, 10.0..=200.0).text("SSAO-Radius"));
                });
                ui.checkbox(&mut fx.dof, "Tiefenunschärfe in der Ferne");
                ui.add_enabled_ui(fx.dof, |ui| {
                    ui.add(egui::Slider::new(&mut fx.dof_strength, 0.0..=1.0).text("Unschärfe-Stärke"));
                    ui.add(egui::Slider::new(&mut fx.dof_start, 2000.0..=30000.0).text("Beginn der Unschärfe"));
                    if fx.dof_end <= fx.dof_start {
                        fx.dof_end = (fx.dof_start + 1000.0).min(32000.0);
                    }
                });
                if ui.small_button("Effekte auf Standard zurücksetzen").clicked() {
                    *fx = em4r_core::config::Fx { enabled: true, capture: fx.capture, ..Default::default() };
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Leistung").strong());
            let fps_label = |v: u32| if v == 0 { "unbegrenzt".to_string() } else { format!("{v} FPS") };
            egui::ComboBox::from_label("FPS-Limit").selected_text(fps_label(r.cfg.perf.frame_limit)).show_ui(ui, |ui| {
                for v in [0u32, 30, 60, 75, 120, 144, 165, 240] {
                    ui.selectable_value(&mut r.cfg.perf.frame_limit, v, fps_label(v));
                }
            });
            let vs_label = |v: i32| match v {
                0 => "aus",
                1 => "an",
                _ => "Spiel entscheidet",
            };
            egui::ComboBox::from_label("VSync").selected_text(vs_label(r.cfg.perf.vsync)).show_ui(ui, |ui| {
                for v in [-1, 0, 1] {
                    ui.selectable_value(&mut r.cfg.perf.vsync, v, vs_label(v));
                }
            });
            ui.checkbox(&mut r.cfg.game.fast_logfile, "Schnelles Log (spart ~5 s Ladezeit)");
            ui.checkbox(&mut r.cfg.perf.missing_file_cache, "Speicher für fehlende Dateien (weniger Dateisuchen)");
            ui.checkbox(&mut r.cfg.perf.fast_zlib, "Schnelles Entpacken (modernes zlib statt Version von 2003)");
            ui.checkbox(&mut r.cfg.perf.high_res_timer, "Genauer Windows-Timer (gleichmäßigere Bildrate)");

            ui.add_space(8.0);
            ui.label(RichText::new("Spiel").strong());
            ui.checkbox(&mut r.cfg.game.skip_intro, "Intro überspringen");
            ui.checkbox(&mut r.cfg.game.fast_exit, "Schnelles Beenden (Einstellungen speichern, sofort schließen)");

            ui.add_space(8.0);
            ui.label(RichText::new("Fenster").strong());
            ui.checkbox(&mut r.cfg.window.borderless, "Randloses Vollbild (schnelles Alt+Tab, Screenshots funktionieren)")
                .on_hover_text("Keine Auflösungsumschaltung und kein Minimieren beim Wechseln in andere Fenster.");
            if r.cfg.window.borderless && self.fullscreen {
                if let Some(desk) = em4cfg::desktop_mode() {
                    if desk != self.resolution {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                WARN,
                                format!(
                                    "Greift nur bei Desktop-Auflösung ({} × {}), eingestellt ist {} × {}.",
                                    desk.0, desk.1, self.resolution.0, self.resolution.1
                                ),
                            );
                            if ui.small_button("Desktop-Auflösung übernehmen").clicked() {
                                self.resolution = desk;
                                if !self.modes.contains(&desk) {
                                    self.modes.push(desk);
                                    self.modes.sort();
                                }
                            }
                        });
                    }
                }
            }
            ui.add_enabled(
                r.cfg.window.borderless,
                egui::Checkbox::new(&mut r.cfg.window.pause_in_background, "Im Hintergrund pausieren"),
            );
            ui.checkbox(&mut r.cfg.window.allow_system_keys, "Druck-Taste und Windows-Taste im Spiel nicht sperren");

            ui.add_space(8.0);
            ui.label(RichText::new("Diagnose").strong());
            ui.checkbox(&mut r.cfg.stats.enabled, "FPS-Messung (em4revive_stats.csv)");
            ui.checkbox(&mut r.cfg.crash.minidump, "Absturzberichte schreiben");
            ui.checkbox(&mut r.cfg.profiler.enabled, "Profiler (nur zur Fehlersuche – kostet Leistung)");
        });
    }

    fn game_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Spieleinstellungen (em4.cfg)", |ui| {
            if let Err(e) = &self.em4 {
                ui.colored_label(WARN, e.as_str());
                return;
            }
            let res = |r: (u32, u32)| format!("{} × {}", r.0, r.1);
            egui::ComboBox::from_label("Auflösung").selected_text(res(self.resolution)).show_ui(ui, |ui| {
                for m in self.modes.clone() {
                    ui.selectable_value(&mut self.resolution, m, res(m));
                }
            });
            ui.checkbox(&mut self.fullscreen, "Vollbild");
            ui.checkbox(&mut self.shadows, "Schatten");
            ui.checkbox(&mut self.bloom, "Bloom (Leuchteffekt)");
            ui.label(
                RichText::new("Vor der ersten Änderung wird em4.cfg nach _em4revive_backup gesichert.")
                    .small()
                    .color(MUTED),
            );
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(CARD).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width().min(760.0));
        ui.label(RichText::new(title).size(18.0).strong().color(Color32::WHITE));
        ui.add_space(6.0);
        body(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_game_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("em4r_settings_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("em4.cfg"),
            "<config>\n<var name=\"e4_bloom\" value=\"0\" />\n<var name=\"e4_shadow\" value=\"1\" />\n<var name=\"r_fullscreen\" value=\"1\" />\n<var name=\"r_xres\" value=\"1920\" />\n<var name=\"r_yres\" value=\"1080\" />\n</config>\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn unsaved_changes_follow_edit_save_and_reload() {
        let dir = fake_game_dir("flow");
        let mut p = SettingsPage::new(&dir);
        assert!(!p.has_unsaved_changes());

        // Game option: changing it back counts as no change.
        p.bloom = true;
        assert!(p.has_unsaved_changes());
        p.bloom = false;
        assert!(!p.has_unsaved_changes());

        p.bloom = true;
        assert!(p.save());
        assert!(!p.has_unsaved_changes());
        assert!(SettingsPage::new(&dir).bloom);

        // Mod option, discarded by reloading.
        p.revive.as_mut().unwrap().cfg.game.skip_intro ^= true;
        assert!(p.has_unsaved_changes());
        p.reload();
        assert!(!p.has_unsaved_changes());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_save_keeps_changes_marked_as_unsaved() {
        let dir = fake_game_dir("fail");
        let mut p = SettingsPage::new(&dir);
        p.bloom = true;
        // A directory in place of the file makes the write fail.
        std::fs::remove_file(dir.join("em4.cfg")).unwrap();
        std::fs::create_dir(dir.join("em4.cfg")).unwrap();
        assert!(!p.save());
        assert!(p.has_unsaved_changes());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
