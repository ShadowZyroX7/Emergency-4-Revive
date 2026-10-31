//! The launcher window.

use crate::mods::{self, ModInfo};
use crate::session::{self, Ending, Running, Summary, Target};
use crate::settings::{self, Settings};
use crate::settings_page::SettingsPage;
use em4r_core::launch::StartMode;
use eframe::egui::{self, Color32, RichText, Vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ACCENT: Color32 = Color32::from_rgb(214, 48, 49); // emergency red
const ACCENT_DIM: Color32 = Color32::from_rgb(120, 32, 34);
const CARD: Color32 = Color32::from_rgb(32, 34, 38);
const CARD_SELECTED: Color32 = Color32::from_rgb(58, 36, 38);
const MUTED: Color32 = Color32::from_rgb(150, 154, 160);
const OK: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Spielen,
    Einstellungen,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Original,
    Mod(usize),
}

pub struct LauncherApp {
    game_dir: Option<PathBuf>,
    settings_path: PathBuf,
    settings: Settings,
    mods: Vec<ModInfo>,
    selection: Selection,
    mode: StartMode,
    textures: HashMap<PathBuf, Option<egui::TextureHandle>>,
    running: Option<Running>,
    last: Option<Summary>,
    error: Option<String>,
    revive_status: (bool, String),
    tab: Tab,
    settings_page: Option<SettingsPage>,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.selection.bg_fill = ACCENT;
        visuals.panel_fill = Color32::from_rgb(22, 23, 26);
        visuals.window_fill = Color32::from_rgb(22, 23, 26);
        cc.egui_ctx.set_visuals(visuals);

        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let settings_path = exe_dir.clone().unwrap_or_default().join("em4revive_launcher.toml");
        let settings = Settings::load(&settings_path);
        let game_dir = settings::find_game_dir(exe_dir.as_deref(), settings.game_dir.as_deref());

        let mut app = Self {
            game_dir,
            settings_path,
            settings: settings.clone(),
            mods: Vec::new(),
            selection: Selection::Original,
            mode: settings.last_mode.unwrap_or(StartMode::FreeplayEndless),
            textures: HashMap::new(),
            running: None,
            last: None,
            error: None,
            revive_status: (false, String::new()),
            tab: Tab::Spielen,
            settings_page: None,
        };
        app.refresh();
        // `EM4Revive.exe --einstellungen` opens the settings tab directly.
        if std::env::args().any(|a| a.eq_ignore_ascii_case("--einstellungen")) {
            if let Some(dir) = &app.game_dir {
                app.settings_page = Some(SettingsPage::new(dir));
                app.tab = Tab::Einstellungen;
            }
        }
        app
    }

    fn refresh(&mut self) {
        let Some(dir) = self.game_dir.clone() else {
            self.error = Some("Emergency 4 wurde nicht gefunden. Lege EM4Revive.exe in den Spielordner (neben Em4.exe).".into());
            return;
        };
        self.mods = mods::scan(&dir);
        self.selection = self
            .mods
            .iter()
            .position(|m| m.folder.eq_ignore_ascii_case(&self.settings.last_mod))
            .map(Selection::Mod)
            .unwrap_or(Selection::Original);
        self.revive_status = revive_status(&dir);
    }

    fn refresh_status(&mut self) {
        if let Some(dir) = &self.game_dir {
            self.revive_status = revive_status(dir);
        }
    }

    fn texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        self.textures
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let (w, h, rgba) = load_rgba(path)?;
                let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                Some(ctx.load_texture(path.to_string_lossy(), color, egui::TextureOptions::LINEAR))
            })
            .clone()
    }

    fn selected_target(&self) -> (Target, String) {
        match self.selection {
            Selection::Original => (Target::Original, "Original EMERGENCY 4".into()),
            Selection::Mod(i) => (Target::Mod(self.mods[i].folder.clone()), self.mods[i].name.clone()),
        }
    }

    fn play(&mut self) {
        let Some(dir) = self.game_dir.clone() else { return };
        let (target, label) = self.selected_target();
        self.settings.last_mod = match &target {
            Target::Original => String::new(),
            Target::Mod(f) => f.clone(),
        };
        self.settings.last_mode = Some(self.mode);
        self.settings.save(&self.settings_path);
        match session::launch(&dir, &target, self.mode, &label) {
            Ok(run) => {
                self.running = Some(run);
                self.last = None;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Start fehlgeschlagen: {e}")),
        }
    }

    fn poll_running(&mut self) {
        let finished = match self.running.as_mut() {
            Some(run) => matches!(run.child.try_wait(), Ok(Some(_)) | Err(_)),
            None => false,
        };
        if finished {
            if let (Some(run), Some(dir)) = (self.running.take(), self.game_dir.as_deref()) {
                self.last = Some(session::summarize(dir, &run));
            }
        }
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_running();
        if self.running.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::TopBottomPanel::top("header").exact_height(56.0).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("EM4 REVIVE").size(24.0).strong().color(ACCENT));
                ui.label(RichText::new("Launcher").size(18.0).color(MUTED));
                ui.add_space(24.0);
                for (tab, label) in [(Tab::Spielen, "Spielen"), (Tab::Einstellungen, "Einstellungen")] {
                    let active = self.tab == tab;
                    let text = RichText::new(label).size(16.0).color(if active { Color32::WHITE } else { MUTED });
                    let b = egui::Button::new(text)
                        .fill(if active { CARD_SELECTED } else { Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0_f32, if active { ACCENT } else { Color32::TRANSPARENT }))
                        .min_size(Vec2::new(110.0, 32.0));
                    if ui.add(b).clicked() && self.tab != tab {
                        self.tab = tab;
                        match tab {
                            Tab::Einstellungen => {
                                if let Some(dir) = &self.game_dir {
                                    // Always show the current state of the files.
                                    self.settings_page = Some(SettingsPage::new(dir));
                                }
                            }
                            Tab::Spielen => self.refresh_status(),
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (ok, text) = &self.revive_status;
                    let color = if *ok { OK } else { WARN };
                    ui.label(RichText::new(text).color(color));
                    // Status dot (drawn – the font has no reliable bullet glyph).
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                });
            });
        });

        egui::TopBottomPanel::bottom("footer").exact_height(30.0).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(dir) = &self.game_dir {
                    ui.label(RichText::new(dir.display().to_string()).small().color(MUTED));
                    if ui.small_button("Ordner öffnen").clicked() {
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Neu einlesen").clicked() {
                        self.textures.clear();
                        self.refresh();
                    }
                    ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small().color(MUTED));
                });
            });
        });

        if self.tab == Tab::Einstellungen {
            let running = self.running.is_some();
            egui::CentralPanel::default().show(ctx, |ui| match self.settings_page.as_mut() {
                Some(p) => p.ui(ui, running),
                None => {
                    ui.colored_label(WARN, "Spielordner nicht gefunden.");
                }
            });
            return;
        }

        egui::SidePanel::left("mods").resizable(false).exact_width(340.0).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Mods").size(16.0).strong());
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut clicked = None;
                if self.card(ui, ctx, None, self.selection == Selection::Original) {
                    clicked = Some(Selection::Original);
                }
                for i in 0..self.mods.len() {
                    if self.card(ui, ctx, Some(i), self.selection == Selection::Mod(i)) {
                        clicked = Some(Selection::Mod(i));
                    }
                }
                if let Some(c) = clicked {
                    self.selection = c;
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| self.details(ui, ctx));
    }
}

impl LauncherApp {
    /// One entry in the mod list. Returns true when clicked.
    fn card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: Option<usize>, selected: bool) -> bool {
        let (title, subtitle, preview) = match idx {
            None => ("Original EMERGENCY 4".to_string(), "ohne Mod".to_string(), None),
            Some(i) => {
                let m = &self.mods[i];
                (m.name.clone(), m.author.clone(), m.preview.clone())
            }
        };
        let tex = preview.and_then(|p| self.texture(ctx, &p));
        let frame = egui::Frame::NONE
            .fill(if selected { CARD_SELECTED } else { CARD })
            .stroke(egui::Stroke::new(1.0_f32, if selected { ACCENT } else { Color32::from_gray(45) }))
            .inner_margin(8.0);
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    picture(ui, tex.as_ref(), Vec2::new(56.0, 56.0), "EM4", 16.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(&title).strong().color(Color32::WHITE));
                        ui.label(RichText::new(&subtitle).small().color(MUTED));
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        ui.add_space(6.0);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }

    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(err) = &self.error {
            ui.colored_label(WARN, err);
            ui.add_space(8.0);
        }
        if self.game_dir.is_none() {
            return;
        }
        let (title, author, version, folder, maps, campaign, preview) = match self.selection {
            Selection::Original => (
                "Original EMERGENCY 4".to_string(),
                "Sixteen Tons Entertainment".to_string(),
                String::new(),
                None,
                Vec::new(),
                true,
                None,
            ),
            Selection::Mod(i) => {
                let m = &self.mods[i];
                (
                    m.name.clone(),
                    m.author.clone(),
                    m.version.clone(),
                    Some(m.folder.clone()),
                    m.maps.clone(),
                    m.has_campaign,
                    m.preview.clone(),
                )
            }
        };

        ui.add_space(12.0);
        let tex = preview.and_then(|p| self.texture(ctx, &p));
        // Banner: logo (mod previews are small 128 px logos, so keep them
        // at a modest size) next to title, author, version and maps.
        egui::Frame::NONE.fill(CARD).inner_margin(16.0).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            ui.horizontal(|ui| {
                picture(ui, tex.as_ref(), Vec2::new(176.0, 176.0), "EM4", 40.0);
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new(&title).size(28.0).strong().color(Color32::WHITE));
                    ui.add_space(2.0);
                    ui.label(RichText::new(&author).color(MUTED));
                    let mut meta = Vec::new();
                    if !version.is_empty() {
                        meta.push(version.clone());
                    }
                    if let Some(f) = &folder {
                        meta.push(format!("Ordner: {f}"));
                    }
                    if !meta.is_empty() {
                        ui.label(RichText::new(meta.join("  ·  ")).small().color(MUTED));
                    }
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        for m in &maps {
                            chip(ui, &format!("Karte: {m}"));
                        }
                        if campaign {
                            chip(ui, "Kampagne");
                        }
                    });
                });
            });
        });

        ui.add_space(14.0);
        ui.label(RichText::new("Starten in").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for m in [StartMode::FreeplayEndless, StartMode::FreeplayChallenge, StartMode::Menu] {
                let selected = self.mode == m;
                let text = RichText::new(m.label()).color(if selected { Color32::WHITE } else { MUTED });
                let b = egui::Button::new(text)
                    .fill(if selected { CARD_SELECTED } else { CARD })
                    .stroke(egui::Stroke::new(1.0_f32, if selected { ACCENT } else { Color32::from_gray(50) }))
                    .min_size(Vec2::new(0.0, 32.0));
                if ui.add(b).clicked() {
                    self.mode = m;
                }
            }
        });

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let running = self.running.is_some();
            let label = if running { "Spiel läuft …" } else { "▶  SPIELEN" };
            let button = egui::Button::new(RichText::new(label).size(20.0).strong().color(Color32::WHITE))
                .fill(if running { ACCENT_DIM } else { ACCENT });
            if ui.add_enabled(!running, button.min_size(Vec2::new(220.0, 48.0))).clicked() {
                self.play();
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                let hint = match self.mode {
                    StartMode::Menu => "Startet ins Hauptmenü der Mod.",
                    _ => "Lädt die Karte automatisch, sobald das Hauptmenü bereit ist.",
                };
                ui.label(RichText::new(hint).color(MUTED));
                ui.label(RichText::new("Ohne Intro – der Mod-Wechsel im Spiel entfällt.").small().color(MUTED));
            });
        });

        ui.add_space(14.0);
        if let Some(run) = &self.running {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                let s = run.started.elapsed().as_secs();
                ui.label(format!("{} läuft seit {}:{:02}", run.target_label, s / 60, s % 60));
            });
        } else if let Some(sum) = &self.last {
            summary_box(ui, sum);
        }
    }
}

/// Decode JPG/PNG via `image`, DDS via our own decoder.
fn load_rgba(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let is_dds = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds"));
    if is_dds {
        crate::dds::decode(&std::fs::read(path).ok()?)
    } else {
        let img = image::open(path).ok()?.to_rgba8();
        Some((img.width(), img.height(), img.into_raw()))
    }
}

/// Largest size with the texture's aspect ratio that fits into `bounds`.
fn fit(tex: &egui::TextureHandle, bounds: Vec2) -> Vec2 {
    let s = tex.size_vec2();
    let k = (bounds.x / s.x).min(bounds.y / s.y);
    s * k
}

/// Draw `tex` centered in a fixed box (or a placeholder), keeping aspect.
fn picture(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, bounds: Vec2, placeholder: &str, font: f32) {
    let (rect, _) = ui.allocate_exact_size(bounds, egui::Sense::hover());
    match tex {
        Some(t) => {
            let r = egui::Rect::from_center_size(rect.center(), fit(t, bounds));
            egui::Image::new(egui::load::SizedTexture::from_handle(t)).paint_at(ui, r);
        }
        None => {
            ui.painter().rect_filled(rect, 3.0, ACCENT_DIM);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                placeholder,
                egui::FontId::proportional(font),
                Color32::WHITE,
            );
        }
    }
}

fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(40, 42, 47))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(Color32::from_gray(210)));
        });
}

fn summary_box(ui: &mut egui::Ui, s: &Summary) {
    egui::Frame::NONE.fill(CARD).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width().min(720.0));
        ui.label(RichText::new("Letzte Sitzung").strong());
        let fps = match (s.fps_avg, s.fps_low) {
            (Some(a), Some(l)) => format!("Ø {a:.0} FPS (1%-Low {l:.0})"),
            _ => "keine FPS-Messung (EM4 Revive aus?)".into(),
        };
        ui.label(format!("{}  ·  {:.0} min  ·  {}", s.label, s.minutes, fps));
        match &s.ending {
            Ending::Clean => {
                ui.colored_label(OK, "Sauber beendet.");
            }
            Ending::CrashOnExit => {
                ui.colored_label(
                    WARN,
                    "Beim Beenden abgestürzt (bekannter Fehler des Spiels bzw. der Mod – Spielfortschritt nicht betroffen, \
                     aber im Spiel geänderte Optionen wurden evtl. nicht gespeichert).",
                );
            }
            Ending::Crash(dump) => {
                let extra = dump
                    .as_ref()
                    .and_then(|d| d.file_name())
                    .map(|n| format!(" Absturzbericht: {}", n.to_string_lossy()))
                    .unwrap_or_default();
                ui.colored_label(ACCENT, format!("Das Spiel ist während des Spielens abgestürzt.{extra}"));
            }
        }
    });
}

/// "EM4 Revive aktiv" if the proxy DLL is installed and enabled.
fn revive_status(dir: &Path) -> (bool, String) {
    if !dir.join("dinput8.dll").is_file() {
        return (false, "EM4 Revive nicht installiert".into());
    }
    let enabled = std::fs::read_to_string(dir.join("em4revive.toml"))
        .ok()
        .and_then(|t| em4r_core::Config::parse(t.trim_start_matches('\u{feff}')).ok())
        .map(|c| c.general.enabled)
        .unwrap_or(true);
    if enabled {
        (true, "EM4 Revive aktiv".into())
    } else {
        (false, "EM4 Revive deaktiviert (em4revive.toml)".into())
    }
}
