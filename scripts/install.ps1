# Installs EM4 Revive into the game folder.
# Copies only dinput8.dll (+ em4revive.toml if not present). No game file is modified.
param(
    [string]$GameDir = 'C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe',
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dll = Join-Path $root 'dist\dinput8.dll'
if (-not (Test-Path $dll)) { throw "dist\dinput8.dll fehlt – zuerst scripts\build.ps1 ausführen." }
if (-not (Test-Path (Join-Path $GameDir 'Em4.exe'))) { throw "Em4.exe nicht gefunden in $GameDir" }
if (Get-Process Em4 -ErrorAction SilentlyContinue) { throw "Emergency 4 läuft noch – bitte erst beenden." }

$backup = Join-Path $GameDir '_em4revive_backup'
$marker = Join-Path $backup 'installed.txt'
New-Item -ItemType Directory -Force $backup | Out-Null

# Save original config once (never overwritten later).
foreach ($f in 'em4.cfg', 'ReShade.ini') {
    $src = Join-Path $GameDir $f
    if ((Test-Path $src) -and -not (Test-Path (Join-Path $backup $f))) { Copy-Item $src $backup }
}

# Never silently replace a foreign dinput8.dll (another mod).
$target = Join-Path $GameDir 'dinput8.dll'
if ((Test-Path $target) -and -not (Test-Path $marker)) {
    if (-not $Force) { throw "Fremde dinput8.dll im Spielordner gefunden. Mit -Force wird sie nach _em4revive_backup gesichert und ersetzt." }
    Copy-Item $target (Join-Path $backup 'dinput8.dll.fremd') -Force
}

Copy-Item $dll $target -Force
$pdb = Join-Path $root 'dist\dinput8.pdb'
if (Test-Path $pdb) { Copy-Item $pdb $GameDir -Force }
$cfg = Join-Path $GameDir 'em4revive.toml'
if (-not (Test-Path $cfg)) { Copy-Item (Join-Path $root 'dist\em4revive.toml') $cfg }
$launcher = Join-Path $root 'dist\EM4Revive.exe'
if (Test-Path $launcher) {
    if (Get-Process EM4Revive -ErrorAction SilentlyContinue) { throw "Der EM4 Revive Launcher läuft noch – bitte erst schließen." }
    Copy-Item $launcher $GameDir -Force
}

$hash = (Get-FileHash $target -Algorithm SHA256).Hash
Set-Content $marker -Encoding utf8 -Value "installiert=$(Get-Date -Format s)`nsha256=$hash"
Write-Host "EM4 Revive installiert. Log nach dem Spielstart: $GameDir\em4revive.log" -ForegroundColor Green
