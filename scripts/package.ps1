# Baut dist\EM4Revive-Setup.exe. Das GitHub-Projekt wird fest in den Launcher
# eingebaut (daraus holt er seine Updates): -Repo "name/projekt" oder automatisch
# aus dem Git-Remote "origin". Ohne Projekt entsteht ein Setup ohne Auto-Update.
param(
    [string]$Repo,
    [switch]$NoTests
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$env:Path += ";$env:USERPROFILE\.cargo\bin"

if (-not $Repo) {
    $origin = git -C $root config --get remote.origin.url
    if ($origin -match 'github\.com[:/]([^/]+/[^/.]+?)(\.git)?/?$') { $Repo = $Matches[1] }
}
if ($Repo) {
    if ($Repo -notmatch '^[\w.-]+/[\w.-]+$') { throw "Ungültiges GitHub-Projekt: $Repo (erwartet: name/projekt)" }
    $env:EM4R_GITHUB_REPO = $Repo
    Write-Host "Updates kommen von: github.com/$Repo" -ForegroundColor Cyan
} else {
    Remove-Item Env:EM4R_GITHUB_REPO -ErrorAction SilentlyContinue
    Write-Warning "Kein GitHub-Projekt bekannt: dieses Setup enthält KEINE automatischen Updates."
}

& "$PSScriptRoot\build.ps1" -NoTests:$NoTests

Push-Location $root
try {
    $env:EM4R_PAYLOAD_DIR = Join-Path $root 'dist'
    cargo build --release -p em4r-setup
    if ($LASTEXITCODE -ne 0) { throw "Setup-Build fehlgeschlagen" }
    $setup = Join-Path $root 'dist\EM4Revive-Setup.exe'
    Copy-Item "$root\target\i686-pc-windows-msvc\release\EM4Revive-Setup.exe" $setup -Force
    $mb = [math]::Round((Get-Item $setup).Length / 1MB, 1)
    Write-Host "Fertig: $setup ($mb MB)" -ForegroundColor Green
} finally {
    Remove-Item Env:EM4R_PAYLOAD_DIR -ErrorAction SilentlyContinue
    Pop-Location
}
