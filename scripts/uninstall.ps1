# Removes EM4 Revive from the game folder and restores the original state.
param(
    [string]$GameDir = 'C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe',
    [switch]$KeepLogs
)
$ErrorActionPreference = 'Stop'
if (Get-Process Em4 -ErrorAction SilentlyContinue) { throw "Emergency 4 läuft noch – bitte erst beenden." }
$backup = Join-Path $GameDir '_em4revive_backup'
$marker = Join-Path $backup 'installed.txt'
if (-not (Test-Path $marker)) { throw "EM4 Revive scheint nicht installiert zu sein (kein $marker)." }

$target = Join-Path $GameDir 'dinput8.dll'
$expected = ((Get-Content $marker) -match '^sha256=') -replace '^sha256=', ''
if (Test-Path $target) {
    $actual = (Get-FileHash $target -Algorithm SHA256).Hash
    if ($actual -ne $expected) {
        Write-Warning "dinput8.dll wurde seit der Installation verändert – sie wird trotzdem entfernt, aber nach _em4revive_backup gesichert."
        Copy-Item $target (Join-Path $backup "dinput8.dll.$actual") -Force
    }
    Remove-Item $target -Force
}
Remove-Item (Join-Path $GameDir 'dinput8.pdb') -Force -ErrorAction SilentlyContinue

$foreign = Join-Path $backup 'dinput8.dll.fremd'
if (Test-Path $foreign) { Move-Item $foreign $target; Write-Host "Vorherige fremde dinput8.dll wiederhergestellt." }

Remove-Item (Join-Path $GameDir 'EM4Revive.exe'), (Join-Path $GameDir 'em4revive_launcher.toml') -Force -ErrorAction SilentlyContinue

if (-not $KeepLogs) {
    Remove-Item (Join-Path $GameDir 'em4revive.log'), (Join-Path $GameDir 'em4revive_stats.csv') -Force -ErrorAction SilentlyContinue
}
Remove-Item $marker -Force
Write-Host "EM4 Revive entfernt. em4revive.toml und Crash-Dumps wurden behalten (können gelöscht werden)." -ForegroundColor Green
