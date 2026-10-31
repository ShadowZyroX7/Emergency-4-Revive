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

## Bauen & Installieren
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
