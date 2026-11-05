//! The "Submods" tab. It lives as long as the launcher, so a running download
//! continues while the user looks at other tabs.

use crate::github::CANCELLED;
use crate::submods::{self, Installed, Latest, Progress, SubmodDef, SUBMODS};
use eframe::egui::{self, Color32, RichText, Vec2};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const ACCENT: Color32 = Color32::from_rgb(214, 48, 49);
const MUTED: Color32 = Color32::from_rgb(150, 154, 160);
const OK: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);

#[derive(Debug, Clone, PartialEq)]
enum Job {
    Idle,
    Checking,
    Running(Progress),
    /// Finished; where a foreign folder was moved to.
    Done(Option<PathBuf>),
}

struct Shared {
    job: Job,
    latest: Option<Latest>,
    /// A release was found to not exist (as opposed to "not checked yet").
    no_release: bool,
    error: Option<String>,
}

struct Entry {
    def: &'static SubmodDef,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    installed: Installed,
    /// The user has to agree before a foreign folder is moved away.
    confirming: bool,
    moved_to: Option<PathBuf>,
}

pub struct SubmodsPage {
    game_dir: PathBuf,
    entries: Vec<Entry>,
    repaint: egui::Context,
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes / (1024 * 1024))
}

impl SubmodsPage {
    pub fn new(game_dir: &Path, repaint: egui::Context) -> Self {
        let entries = SUBMODS
            .iter()
            .map(|def| Entry {
                def,
                shared: Arc::new(Mutex::new(Shared { job: Job::Idle, latest: None, no_release: false, error: None })),
                cancel: Arc::new(AtomicBool::new(false)),
                installed: submods::installed(game_dir, def),
                confirming: false,
                moved_to: None,
            })
            .collect();
        let page = Self { game_dir: game_dir.to_path_buf(), entries, repaint };
        for index in 0..page.entries.len() {
            page.check(index);
        }
        page
    }

    /// A job is running; the launcher must not close itself for an update.
    pub fn busy(&self) -> bool {
        self.entries.iter().any(|e| matches!(e.shared.lock().unwrap().job, Job::Running(_)))
    }

    fn check(&self, index: usize) {
        let entry = &self.entries[index];
        {
            let mut s = entry.shared.lock().unwrap();
            s.job = Job::Checking;
            s.error = None;
        }
        let (def, shared, repaint) = (entry.def, entry.shared.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let result = submods::latest(def);
            let mut s = shared.lock().unwrap();
            s.job = Job::Idle;
            match result {
                Ok(latest) => {
                    s.no_release = latest.is_none();
                    s.latest = latest;
                }
                Err(e) => s.error = Some(format!("GitHub nicht erreichbar: {e}")),
            }
            repaint.request_repaint();
        });
    }

    fn start(&self, index: usize, latest: Latest) {
        let entry = &self.entries[index];
        entry.cancel.store(false, Ordering::Relaxed);
        {
            let mut s = entry.shared.lock().unwrap();
            s.error = None;
            s.job = Job::Running(Progress::Download { done: 0, total: latest.size });
        }
        let (def, shared, cancel) = (entry.def, entry.shared.clone(), entry.cancel.clone());
        let (game_dir, repaint) = (self.game_dir.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let mut shown = (0u8, 0u64);
            let report = |p: Progress, shown: &mut (u8, u64)| {
                // One repaint per percent is plenty.
                let key = match p {
                    Progress::Download { done, total } => (1, done * 100 / total.max(1)),
                    Progress::Unpack { done, total } => (2, done * 100 / total.max(1)),
                    Progress::Install => (3, 0),
                };
                if key != *shown {
                    *shown = key;
                    shared.lock().unwrap().job = Job::Running(p);
                    repaint.request_repaint();
                }
            };
            let result = submods::install(&game_dir, def, &latest, &cancel, |p| report(p, &mut shown));
            let mut s = shared.lock().unwrap();
            match result {
                Ok(moved) => s.job = Job::Done(moved),
                Err(e) => {
                    s.job = Job::Idle;
                    if e != CANCELLED {
                        s.error = Some(e);
                    }
                }
            }
            repaint.request_repaint();
        });
    }

    /// Called every frame, on any tab. Returns true when a submod was just
    /// installed, so the mod list can pick it up.
    pub fn poll(&mut self) -> bool {
        let mut installed_now = false;
        for entry in &mut self.entries {
            let mut s = entry.shared.lock().unwrap();
            if let Job::Done(moved) = &s.job {
                entry.moved_to = moved.clone();
                s.job = Job::Idle;
                drop(s);
                entry.installed = submods::installed(&self.game_dir, entry.def);
                entry.confirming = false;
                installed_now = true;
            }
        }
        installed_now
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_running: bool) {
        ui.add_space(8.0);
        ui.label(RichText::new("Submods").size(20.0).strong());
        ui.label(RichText::new("Zusätzliche Mods, direkt vom jeweiligen GitHub-Projekt.").color(MUTED));
        ui.add_space(10.0);
        for index in 0..self.entries.len() {
            self.card(ui, index, game_running);
            ui.add_space(8.0);
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, index: usize, game_running: bool) {
        let (job, latest, no_release, error) = {
            let s = self.entries[index].shared.lock().unwrap();
            (s.job.clone(), s.latest.clone(), s.no_release, s.error.clone())
        };

        let mut action: Option<Action> = None;
        egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let entry = &mut self.entries[index];
            ui.horizontal(|ui| {
                ui.label(RichText::new(entry.def.name).size(17.0).strong());
                ui.hyperlink_to("GitHub", format!("https://github.com/{}", entry.def.repo));
            });
            ui.label(entry.def.description);
            ui.add_space(6.0);

            match &entry.installed {
                Installed::No => ui.label(RichText::new("Nicht installiert").color(MUTED)),
                Installed::Foreign => ui.colored_label(WARN, "Ordner vorhanden, aber nicht über den Launcher installiert"),
                Installed::Version(v) => ui.colored_label(OK, format!("Installiert: {v}")),
            };
            match (&latest, &job) {
                (Some(l), _) => ui.label(format!("Neueste Version: {} ({})", l.tag, megabytes(l.size))),
                (None, Job::Checking) => ui.label(RichText::new("Suche neueste Version…").color(MUTED)),
                (None, _) if no_release => ui.label(RichText::new("Noch keine Version veröffentlicht").color(MUTED)),
                _ => ui.label(""),
            };
            if let Some(path) = &entry.moved_to {
                ui.colored_label(WARN, format!("Dein bisheriger Ordner liegt jetzt unter:\n{}", path.display()));
            }
            if let Some(e) = &error {
                ui.colored_label(ACCENT, e);
            }
            ui.add_space(6.0);

            if let Job::Running(progress) = job {
                let (text, fraction, can_cancel) = match progress {
                    Progress::Download { done, total } => (
                        format!("Wird geladen… {} von {}", megabytes(done), megabytes(total)),
                        done as f32 / total.max(1) as f32,
                        true,
                    ),
                    Progress::Unpack { done, total } => {
                        ("Wird entpackt…".to_owned(), done as f32 / total.max(1) as f32, true)
                    }
                    Progress::Install => ("Wird eingerichtet…".to_owned(), 1.0, false),
                };
                ui.add(egui::ProgressBar::new(fraction).text(text).fill(ACCENT));
                if can_cancel && ui.button("Abbrechen").clicked() {
                    entry.cancel.store(true, Ordering::Relaxed);
                }
                return;
            }

            let Some(latest) = latest else {
                if ui.small_button("Erneut suchen").clicked() {
                    action = Some(Action::Check);
                }
                return;
            };
            let up_to_date = matches!(&entry.installed, Installed::Version(v) if *v == latest.tag);
            if up_to_date {
                ui.horizontal(|ui| {
                    ui.colored_label(OK, "Auf dem neuesten Stand");
                    if ui.small_button("Erneut suchen").clicked() {
                        action = Some(Action::Check);
                    }
                });
                return;
            }

            if entry.confirming {
                ui.colored_label(
                    WARN,
                    format!(
                        "Der vorhandene Ordner „{}“ wird unverändert nach _em4revive_backup\\Submods verschoben \
                         und durch die Version {} ersetzt. Spielstände werden übernommen, alles andere \
                         (auch eigene Änderungen) bleibt nur in der Sicherung.",
                        entry.def.folder, latest.tag
                    ),
                );
                ui.horizontal(|ui| {
                    if ui.add_enabled(!game_running, egui::Button::new("Verschieben und herunterladen")).clicked() {
                        entry.confirming = false;
                        action = Some(Action::Start(latest.clone()));
                    }
                    if ui.button("Abbrechen").clicked() {
                        entry.confirming = false;
                    }
                });
            } else {
                let label = if matches!(entry.installed, Installed::Version(_)) { "Aktualisieren" } else { "Herunterladen" };
                let button = egui::Button::new(RichText::new(label).size(15.0).color(Color32::WHITE))
                    .fill(ACCENT)
                    .min_size(Vec2::new(150.0, 32.0));
                if ui.add_enabled(!game_running, button).clicked() {
                    if entry.installed == Installed::Foreign {
                        entry.confirming = true;
                    } else {
                        action = Some(Action::Start(latest.clone()));
                    }
                }
                if game_running {
                    ui.label(RichText::new("Bitte erst das Spiel beenden.").color(MUTED));
                } else if matches!(entry.installed, Installed::Version(_)) {
                    ui.label(
                        RichText::new("Spielstände bleiben erhalten, eigene Änderungen im Mod-Ordner werden ersetzt.")
                            .color(MUTED),
                    );
                }
            }
        });

        match action {
            Some(Action::Check) => self.check(index),
            Some(Action::Start(latest)) => {
                self.entries[index].moved_to = None;
                self.start(index, latest);
            }
            None => {}
        }
    }
}

enum Action {
    Check,
    Start(Latest),
}
