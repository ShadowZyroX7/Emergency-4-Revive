# EM4 Revive

Performance- und Grafik-Mod für **Emergency 4 Deluxe**, geschrieben in Rust.
Kein Nachbau: Das Originalspiel (`Em4.exe`) bleibt unverändert. Eine kleine
`dinput8.dll` wird beim Start mitgeladen und verbessert das Spiel von innen.

## Was sie aktuell macht (v0.1)
| Feature | Wirkung |
|---|---|
| Tiefenpuffer 16 → 24 Bit + 8 Bit Stencil | Weniger Flackern/Z-Fighting auf Distanz, bessere ReShade-Tiefeneffekte |
| 16x anisotrope Filterung, Trilinear | Schärfere Straßen und Böden im schrägen Blickwinkel |
| Frame-Limiter, VSync-Steuerung, 1-ms-Timer | Gleichmäßigere Frametimes |
| FPS/Frametime/Draw-Call/RAM-Messung | `em4revive_stats.csv` zum Vergleichen (Excel-tauglich) |
| Crash-Dumps | `em4revive_crash_*.dmp` bei Abstürzen |

Alles ist in `em4revive.toml` einzeln schaltbar. Funktioniert zusammen mit ReShade.

## Installation für Spieler
`EM4Revive-Setup.exe` aus den [Releases](../../releases/latest) herunterladen und starten.
Das Setup findet den Spielordner über Steam. Danach hält sich der Launcher selbst aktuell:
Bei jedem Start sucht er nach einer neuen Version und installiert sie (abschaltbar unten im Launcher).
Entfernen: Windows-Einstellungen → Apps → EM4 Revive.

## Submods
Der Launcher-Tab „Submods“ lädt zusätzliche Mods von ihrem GitHub-Projekt und hält sie aktuell.
Die Liste steht in `crates/em4r-launcher/src/submods.rs` (`SUBMODS`). Ein Release einer Submod braucht
ein `.7z`-Archiv mit der `e4mod.info` im Hauptverzeichnis oder in genau einem Unterordner; jede neue
Versionsnummer (Tag) wird als Update angeboten. Spielstände aus `Save` bleiben bei Updates erhalten.

## Neue Version veröffentlichen
```powershell
.\scripts\release.ps1 -Version 0.3.0
```
Das setzt die Versionsnummer, committet und pusht den Tag. GitHub Actions baut daraufhin das Setup
und erstellt das Release (`.github/workflows/release.yml`). Das Projekt muss öffentlich sein, sonst
erreichen die Launcher es nicht.

Nur das Setup bauen: `.\scripts\package.ps1` (nimmt das GitHub-Projekt aus dem Git-Remote `origin`
oder aus `-Repo name/projekt`). Soll ein Release von Hand hochgeladen werden: Der Tag muss `vX.Y.Z`
heißen, die Datei genau `EM4Revive-Setup.exe`, und ihre Version muss zum Tag passen.

## Bauen & Installieren (Entwicklung)
```powershell
.\scripts\build.ps1      # Tests + Release-Build nach dist\
.\scripts\install.ps1    # kopiert dinput8.dll + em4revive.toml ins Spiel
.\scripts\uninstall.ps1  # entfernt alles wieder
```
Voraussetzungen: Rust (Target `i686-pc-windows-msvc`), VS Build Tools (C++).

## Wie es funktioniert
Die Spielmodule (`vBase71.dll`, `vision71.dll`, `Em4.exe`) rufen Windows-/OpenGL-
Funktionen über ihre Import-Tabellen auf. EM4 Revive biegt nur diese Einträge
auf eigene Funktionen um, die das Original aufrufen (IAT-Hooking). ReShade
(`opengl32.dll`) und Windows bleiben unangetastet.

| Crate | Inhalt |
|---|---|
| `em4r-core` | Win32-Bindings, Config, Log, IAT-Patching, Crash-Handler |
| `em4r-gl` | Pixelformat-Fix, Texturfilter, Draw-Call-Zähler |
| `em4r-perf` | Frame-Pacing, Limiter, Statistik |
| `em4r-proxy` | `dinput8.dll`: Laden, Weiterleitung an System-DirectInput |
