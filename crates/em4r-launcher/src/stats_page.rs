//! The "Statistik" tab.

use crate::stats::{self, CrashReport, Session, LOADING_FPS};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const ACCENT: Color32 = Color32::from_rgb(214, 48, 49);
const MUTED: Color32 = Color32::from_rgb(150, 154, 160);
const CARD: Color32 = Color32::from_rgb(32, 34, 38);
const OK: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const LOW_COLOR: Color32 = Color32::from_rgb(230, 170, 60);

pub struct StatsPage {
    game_dir: PathBuf,
    session: Session,
    crashes: Vec<CrashReport>,
    confirm_delete: bool,
    message: Option<String>,
}

impl StatsPage {
    pub fn new(game_dir: &Path) -> Self {
        Self {
            game_dir: game_dir.to_path_buf(),
            session: stats::load_session(game_dir),
            crashes: stats::crash_reports(game_dir),
            confirm_delete: false,
            message: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            self.session_card(ui);
            ui.add_space(14.0);
            self.crash_card(ui);
            ui.add_space(20.0);
        });
    }

    fn session_card(&self, ui: &mut egui::Ui) {
        card(ui, "Letzte Sitzung", |ui| {
            let s = &self.session;
            if s.samples.is_empty() {
                ui.label(RichText::new("Noch keine Messdaten – einmal über den Launcher spielen.").color(MUTED));
                return;
            }
            let when = s.when.map(ago).unwrap_or_default();
            ui.label(RichText::new(format!("{when} · Dauer {}", mmss(s.duration_s()))).color(MUTED));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let f = |v: Option<f64>, unit: &str| v.map(|v| format!("{v:.0}{unit}")).unwrap_or_else(|| "–".into());
                tile(ui, "Ø FPS", &f(s.avg_fps(), ""));
                tile(ui, "Ø 1%-Low", &f(s.avg_low(), ""));
                tile(ui, "Schlechtester Moment", &f(s.worst_low(), " FPS"));
                tile(ui, "RAM-Spitze", &f(s.ram_peak(), " MB"));
                tile(ui, "Karte geladen in", &s.map_load_s.map(|v| format!("{v} s")).unwrap_or_else(|| "–".into()));
            });
            ui.add_space(10.0);
            fps_chart(ui, s);
            ui.horizontal(|ui| {
                legend(ui, ACCENT, "Ø FPS");
                legend(ui, LOW_COLOR, "1%-Low");
                legend(ui, Color32::from_gray(70), "Laden");
            });
        });
    }

    fn crash_card(&mut self, ui: &mut egui::Ui) {
        let total_mb = self.crashes.iter().map(|c| c.size).sum::<u64>() as f64 / 1_048_576.0;
        let title = format!("Absturzberichte ({})", self.crashes.len());
        let mut delete_now = false;
        card(ui, &title, |ui| {
            if self.crashes.is_empty() {
                ui.colored_label(OK, "Keine Absturzberichte vorhanden.");
                return;
            }
            egui::Grid::new("crashes").striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("Wann").strong());
                ui.label(RichText::new("Absturzstelle").strong());
                ui.label(RichText::new("Bewertung").strong());
                ui.end_row();
                for c in &self.crashes {
                    ui.label(c.when.map(ago).unwrap_or_default());
                    match &c.info {
                        Some(i) => {
                            ui.label(RichText::new(i.location()).monospace());
                            let a = i.assessment();
                            let color = if a.contains("harmlos") { MUTED } else { WARN };
                            ui.colored_label(color, a);
                        }
                        None => {
                            ui.label("–");
                            ui.colored_label(WARN, "Bericht nicht lesbar");
                        }
                    }
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Im Ordner zeigen").clicked() {
                    if let Some(c) = self.crashes.first() {
                        let _ = std::process::Command::new("explorer").arg("/select,").arg(&c.path).spawn();
                    }
                }
                if !self.confirm_delete {
                    if ui.button(format!("Alle löschen ({total_mb:.0} MB)")).clicked() {
                        self.confirm_delete = true;
                    }
                } else {
                    ui.colored_label(WARN, "Wirklich alle Absturzberichte löschen?");
                    if ui.button("Ja, löschen").clicked() {
                        delete_now = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        self.confirm_delete = false;
                    }
                }
            });
        });
        if delete_now {
            let mut n = 0;
            for c in &self.crashes {
                if std::fs::remove_file(&c.path).is_ok() {
                    n += 1;
                }
            }
            self.crashes = stats::crash_reports(&self.game_dir);
            self.confirm_delete = false;
            self.message = Some(format!("{n} Absturzbericht(e) gelöscht"));
        }
        if let Some(m) = &self.message {
            ui.add_space(6.0);
            ui.colored_label(OK, m);
        }
    }
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(CARD).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width().min(820.0));
        ui.label(RichText::new(title).size(18.0).strong().color(Color32::WHITE));
        ui.add_space(6.0);
        body(ui);
    });
}

fn tile(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::NONE.fill(Color32::from_rgb(40, 42, 47)).inner_margin(10.0).show(ui, |ui| {
        ui.set_min_width(130.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).small().color(MUTED));
            ui.label(RichText::new(value).size(22.0).strong().color(Color32::WHITE));
        });
    });
}

fn legend(ui: &mut egui::Ui, color: Color32, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 2.0, color);
    ui.label(RichText::new(text).small().color(MUTED));
    ui.add_space(8.0);
}

/// FPS over time; loading phases shaded.
fn fps_chart(ui: &mut egui::Ui, s: &Session) {
    let size = Vec2::new(ui.available_width().min(790.0), 200.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_rgb(24, 25, 28));
    let plot = Rect::from_min_max(rect.min + Vec2::new(40.0, 10.0), rect.max - Vec2::new(10.0, 22.0));
    let t_max = s.duration_s().max(1.0);
    // Scale to the 95th percentile: the main menu runs at ~800 FPS and
    // would otherwise squash the in-game curve. Peaks above are clipped.
    let mut fps: Vec<f64> = s.playing().map(|x| x.fps).collect();
    fps.sort_by(|a, b| a.total_cmp(b));
    let p95 = fps.get(((fps.len() as f64) * 0.95) as usize).or(fps.last()).copied().unwrap_or(30.0);
    let y_max = nice_ceiling(p95.max(30.0));
    let x = |t: f64| plot.left() + (t / t_max) as f32 * plot.width();
    let y = |v: f64| plot.bottom() - (v.min(y_max) / y_max) as f32 * plot.height();

    // Grid + labels.
    for k in 0..=4 {
        let v = y_max * k as f64 / 4.0;
        let yy = y(v);
        p.line_segment([Pos2::new(plot.left(), yy), Pos2::new(plot.right(), yy)], Stroke::new(1.0_f32, Color32::from_gray(45)));
        p.text(Pos2::new(plot.left() - 6.0, yy), egui::Align2::RIGHT_CENTER, format!("{v:.0}"), egui::FontId::proportional(11.0), MUTED);
    }
    p.text(
        Pos2::new(plot.right(), rect.bottom() - 4.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("Zeit: {}", mmss(t_max)),
        egui::FontId::proportional(11.0),
        MUTED,
    );
    // Loading phases.
    let mut prev_t = 0.0;
    for sm in &s.samples {
        if sm.fps < LOADING_FPS {
            let r = Rect::from_min_max(Pos2::new(x(prev_t), plot.top()), Pos2::new(x(sm.t), plot.bottom()));
            p.rect_filled(r, 0.0, Color32::from_gray(50).linear_multiply(0.6));
        }
        prev_t = sm.t;
    }
    // Lines (only while playing; gaps at loading screens).
    for (sel, color) in [(0usize, ACCENT), (1usize, LOW_COLOR)] {
        let mut seg: Vec<Pos2> = Vec::new();
        let width: f32 = if sel == 0 { 2.0 } else { 1.2 };
        let flush = |seg: &mut Vec<Pos2>| {
            if seg.len() >= 2 {
                p.add(egui::Shape::line(std::mem::take(seg), Stroke::new(width, color)));
            }
            seg.clear();
        };
        for sm in &s.samples {
            if sm.fps < LOADING_FPS {
                flush(&mut seg);
                continue;
            }
            seg.push(Pos2::new(x(sm.t), y(if sel == 0 { sm.fps } else { sm.low })));
        }
        flush(&mut seg);
    }
}

fn nice_ceiling(v: f64) -> f64 {
    // Multiples of 4 so the four grid steps get round labels.
    for step in [40.0, 60.0, 80.0, 100.0, 120.0, 160.0, 200.0, 240.0, 320.0, 400.0, 600.0, 800.0, 1000.0] {
        if v <= step {
            return step;
        }
    }
    (v / 500.0).ceil() * 500.0
}

fn mmss(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02} min", s / 60, s % 60)
}

/// "vor 5 Minuten", "vor 2 Stunden", "vor 3 Tagen".
fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} Min.", secs / 60),
        3600..=86_399 => format!("vor {} Std.", secs / 3600),
        _ => format!("vor {} Tagen", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(nice_ceiling(79.0), 80.0);
        assert_eq!(nice_ceiling(130.0), 160.0);
        assert_eq!(nice_ceiling(800.0), 800.0);
        assert_eq!(nice_ceiling(1200.0), 1500.0);
        assert_eq!(mmss(125.0), "2:05 min");
        assert_eq!(ago(SystemTime::now()), "gerade eben");
    }
}
