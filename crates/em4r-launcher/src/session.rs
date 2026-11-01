//! Starting Em4.exe and summarising the session afterwards.

use em4r_core::launch::{LaunchRequest, StartMode};
use std::path::{Path, PathBuf};
use crate::process::GameProcess;
use std::time::{Instant, SystemTime};

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Plain EM4 without a mod.
    Original,
    /// A mod folder under `Mods\`.
    Mod(String),
}

/// Command-line arguments for Em4.exe.
///
/// Em4.exe only accepts a mod when there are exactly three arguments:
/// `-game -mod <Ordner>` (verified in its argument parser at 0x479540).
pub fn arguments(target: &Target) -> Vec<String> {
    match target {
        Target::Original => vec!["-game".into()],
        Target::Mod(folder) => vec!["-game".into(), "-mod".into(), folder.clone()],
    }
}

pub struct Running {
    pub process: GameProcess,
    pub started: Instant,
    pub started_wall: SystemTime,
    pub target_label: String,
}

/// `via_steam`: start through Steam (overlay, F12 screenshots); falls back to
/// a direct start when Steam is not installed.
pub fn launch(game_dir: &Path, target: &Target, mode: StartMode, label: &str, via_steam: bool) -> std::io::Result<Running> {
    // The start request for the mod: written just before the start, read
    // and deleted by the mod; "Hauptmenü" means no request at all.
    let request = game_dir.join(em4r_core::launch::FILE_NAME);
    let _ = std::fs::remove_file(&request);
    if mode != StartMode::Menu {
        LaunchRequest::new(mode).write(game_dir)?;
    }
    let args = arguments(target);
    let steam = if via_steam { crate::process::steam_exe() } else { None };
    let spawned = match &steam {
        Some(s) => GameProcess::spawn_via_steam(s, game_dir, &args),
        None => GameProcess::spawn_direct(game_dir, &args),
    };
    let process = match spawned {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&request);
            return Err(e);
        }
    };
    let label = if mode == StartMode::Menu { label.to_string() } else { format!("{label} – {}", mode.label()) };
    Ok(Running { process, started: Instant::now(), started_wall: SystemTime::now(), target_label: label })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// Clean shutdown.
    Clean,
    /// Known EM4/AUBMP bug: crashes while shutting down ("Destroy Network").
    /// Harmless for the game itself, but em4.cfg is not saved.
    CrashOnExit,
    /// Crashed while playing; path of the dump if one was written.
    Crash(Option<PathBuf>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub label: String,
    pub minutes: f64,
    /// Average FPS over intervals where the game was actually rendering.
    pub fps_avg: Option<f64>,
    pub fps_low: Option<f64>,
    pub ending: Ending,
}

/// Average FPS / 1 %-low from `em4revive_stats.csv` (German number format),
/// ignoring loading screens (< 5 FPS).
pub fn fps_from_csv(csv: &str) -> (Option<f64>, Option<f64>) {
    let rows: Vec<(f64, f64)> = csv
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<f64> = l.split(';').map(|v| v.trim().replace(',', ".").parse().unwrap_or(f64::NAN)).collect();
            (c.len() >= 3 && c[1].is_finite() && c[1] >= 5.0).then(|| (c[1], c[2]))
        })
        .collect();
    if rows.is_empty() {
        return (None, None);
    }
    let n = rows.len() as f64;
    (Some(rows.iter().map(|r| r.0).sum::<f64>() / n), Some(rows.iter().map(|r| r.1).sum::<f64>() / n))
}

/// Classify how the session ended from the tail of `logfile.txt`, the tail
/// of the mod's `em4revive.log` and any crash dump written after `since`.
pub fn classify_ending(log_tail: &str, mod_log_tail: &str, new_dump: Option<PathBuf>) -> Ending {
    // EM4 Revive's fast exit saves the config and ends the process cleanly.
    if mod_log_tail.contains("Schnelles Beenden: Einstellungen gespeichert") {
        return Ending::Clean;
    }
    let shutting_down = log_tail.contains("Destroy Network") || log_tail.contains("load time profile end");
    let fully_closed = log_tail.contains("Destroy Filesystem");
    match (new_dump, fully_closed, shutting_down) {
        (_, true, _) => Ending::Clean,
        (_, false, true) => Ending::CrashOnExit,
        (Some(d), false, false) => Ending::Crash(Some(d)),
        // No dump and no shutdown marker: unexpected end (killed or crashed
        // with the mod disabled).
        (None, false, false) => Ending::Crash(None),
    }
}

fn newest_dump_since(game_dir: &Path, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(game_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            n.starts_with("em4revive_crash_") && n.ends_with(".dmp")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(t, _)| *t >= since)
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

fn file_tail(path: &Path, bytes: usize) -> String {
    let Ok(data) = std::fs::read(path) else { return String::new() };
    let start = data.len().saturating_sub(bytes);
    String::from_utf8_lossy(&data[start..]).into_owned()
}

pub fn summarize(game_dir: &Path, run: &Running) -> Summary {
    let minutes = run.started.elapsed().as_secs_f64() / 60.0;
    // Stats only count if written during this session.
    let csv_path = game_dir.join("em4revive_stats.csv");
    let fresh = std::fs::metadata(&csv_path).and_then(|m| m.modified()).is_ok_and(|t| t >= run.started_wall);
    let (fps_avg, fps_low) =
        if fresh { fps_from_csv(&std::fs::read_to_string(&csv_path).unwrap_or_default()) } else { (None, None) };
    let ending = classify_ending(
        &file_tail(&game_dir.join("logfile.txt"), 4096),
        &file_tail(&game_dir.join("em4revive.log"), 2048),
        newest_dump_since(game_dir, run.started_wall),
    );
    Summary { label: run.target_label.clone(), minutes, fps_avg, fps_low, ending }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_arguments_are_exactly_three() {
        assert_eq!(arguments(&Target::Mod("AUBMP".into())), ["-game", "-mod", "AUBMP"]);
        assert_eq!(arguments(&Target::Original), ["-game"]);
    }

    #[test]
    fn fps_ignores_loading_rows() {
        let csv = "zeit_s;fps_avg;fps_1pct_low;x\n1,0;0,5;0,5;1\n2,0;60,0;40,0;1\n3,0;50,0;30,0;1\n";
        let (avg, low) = fps_from_csv(csv);
        assert!((avg.unwrap() - 55.0).abs() < 1e-9);
        assert!((low.unwrap() - 35.0).abs() < 1e-9);
        assert_eq!(fps_from_csv("header\n"), (None, None));
    }

    #[test]
    fn ending_classification() {
        let exit_crash = "!GameLoop : 656 seconds\n--- load time profile end ---\nDestroy Network...\n!WARNING: global listener";
        let clean = "Destroy Network...\nDestroy Audio...\nDestroy Filesystem...";
        let mid_game = "!GameLoop : 3 seconds\n!WARNING: global listener object is still registered!";
        let dump = Some(PathBuf::from("x.dmp"));
        assert_eq!(classify_ending(exit_crash, "", dump.clone()), Ending::CrashOnExit);
        assert_eq!(classify_ending(exit_crash, "", None), Ending::CrashOnExit);
        assert_eq!(classify_ending(clean, "", None), Ending::Clean);
        assert_eq!(classify_ending(mid_game, "", dump.clone()), Ending::Crash(dump));
        assert_eq!(classify_ending(mid_game, "", None), Ending::Crash(None));
        let fast_exit = "INFO  | Schnelles Beenden: Einstellungen gespeichert, Spiel wird beendet";
        assert_eq!(classify_ending("!GameLoop : 12 seconds", fast_exit, None), Ending::Clean);
    }
}
