//! The setup window.

use crate::{files, perform_install, start_launcher, steam, system, VERSION};
use eframe::egui::{self, Color32, RichText, Vec2};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{channel, Receiver};

const ACCENT: Color32 = Color32::from_rgb(214, 48, 49);
const MUTED: Color32 = Color32::from_rgb(150, 154, 160);
const OK: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);

enum Stage {
    Ready,
    Working(Receiver<Result<(), String>>),
    Done,
}

struct SetupApp {
    dir: String,
    desktop_shortcut: bool,
    stage: Stage,
    error: Option<String>,
}

impl SetupApp {
    fn path(&self) -> PathBuf {
        PathBuf::from(self.dir.trim())
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let (dir, desktop, ctx) = (self.path(), self.desktop_shortcut, ctx.clone());
        self.error = None;
        self.stage = Stage::Working(rx);
        std::thread::spawn(move || {
            let result = if system::is_process_running("EM4Revive.exe") {
                Err("Der EM4 Revive Launcher läuft noch. Bitte schließe ihn und versuche es erneut.".to_owned())
            } else {
                perform_install(&dir).and_then(|()| system::create_shortcuts(&dir, desktop)).map_err(|e| e.to_string())
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Stage::Working(rx) = &self.stage {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(()) => self.stage = Stage::Done,
                    Err(e) => {
                        self.error = Some(e);
                        self.stage = Stage::Ready;
                    }
                }
            }
        }
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        ui.label("Spielordner von Emergency 4 Deluxe:");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.dir).desired_width(400.0));
            if ui.button("Durchsuchen…").clicked() {
                if let Some(picked) = rfd::FileDialog::new().pick_folder() {
                    self.dir = picked.display().to_string();
                }
            }
        });
        ui.add_space(4.0);
        let path = self.path();
        let valid = files::is_game_dir(&path);
        if !valid {
            ui.colored_label(WARN, "In diesem Ordner liegt keine Em4.exe.");
        } else {
            match files::installed_version(&path).as_deref() {
                None => ui.colored_label(OK, "Spiel gefunden."),
                Some("") => ui.colored_label(OK, format!("Ältere Version gefunden – wird auf v{VERSION} aktualisiert.")),
                Some(v) => ui.colored_label(OK, format!("Installiert: v{v} – wird durch v{VERSION} ersetzt.")),
            };
        }
        ui.add_space(10.0);
        ui.checkbox(&mut self.desktop_shortcut, "Verknüpfung auf dem Desktop");
        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "Spieldateien werden nicht verändert. Deine Einstellungen bleiben bei einem Update erhalten, \
                 und der Launcher hält sich danach selbst aktuell.",
            )
            .color(MUTED),
        );
        if let Some(e) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(ACCENT, e);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
            let button = egui::Button::new(RichText::new("Installieren").size(16.0).color(Color32::WHITE))
                .fill(ACCENT)
                .min_size(Vec2::new(150.0, 36.0));
            if ui.add_enabled(valid, button).clicked() {
                self.start(ui.ctx());
            }
        });
    }

    fn done(&mut self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        ui.label(RichText::new("EM4 Revive ist installiert.").size(18.0).color(OK));
        ui.add_space(6.0);
        ui.label("Starte das Spiel künftig über den EM4 Revive Launcher.");
        ui.label("Entfernen kannst du die Mod unter Windows-Einstellungen → Apps.");
        ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
            ui.horizontal(|ui| {
                let start = egui::Button::new(RichText::new("Launcher starten").size(16.0).color(Color32::WHITE))
                    .fill(ACCENT)
                    .min_size(Vec2::new(170.0, 36.0));
                if ui.add(start).clicked() {
                    if let Err(e) = start_launcher(&self.path(), &[]) {
                        system::error_box(&format!("Der Launcher konnte nicht gestartet werden:\n\n{e}"));
                    }
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.add(egui::Button::new("Schließen").min_size(Vec2::new(100.0, 36.0))).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("EM4 REVIVE").size(24.0).strong().color(ACCENT));
                ui.label(RichText::new(format!("Setup v{VERSION}")).size(16.0).color(MUTED));
            });
            ui.add_space(14.0);
            match self.stage {
                Stage::Ready => self.form(ui),
                Stage::Working(_) => {
                    ui.add_space(30.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Wird installiert…");
                    });
                }
                Stage::Done => self.done(ui),
            }
        });
    }
}

pub fn run() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EM4 Revive Setup")
            .with_inner_size([560.0, 340.0])
            .with_resizable(false),
        ..Default::default()
    };
    let dir = steam::find_game().map(|d| d.display().to_string()).unwrap_or_default();
    let result = eframe::run_native(
        "EM4 Revive Setup",
        options,
        Box::new(|cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.selection.bg_fill = ACCENT;
            visuals.panel_fill = Color32::from_rgb(22, 23, 26);
            cc.egui_ctx.set_visuals(visuals);
            Ok(Box::new(SetupApp { dir, desktop_shortcut: true, stage: Stage::Ready, error: None }))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            system::error_box(&format!("Das Setup-Fenster konnte nicht geöffnet werden:\n\n{e}"));
            ExitCode::FAILURE
        }
    }
}
