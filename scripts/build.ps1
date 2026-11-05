# Builds the release DLL and collects it in dist\.
param(
    [switch]$NoTests   # for build machines without a graphics driver
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$env:Path += ";$env:USERPROFILE\.cargo\bin"
Push-Location $root
try {
    if (-not $NoTests) {
        cargo test --workspace --quiet
        if ($LASTEXITCODE -ne 0) { throw "Tests fehlgeschlagen" }
    }
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Build fehlgeschlagen" }
    if (-not $NoTests) {
        & "$root\target\i686-pc-windows-msvc\release\em4r-smoke.exe" "$root\target\i686-pc-windows-msvc\release\dinput8.dll" | Select-String '^\['
        if ($LASTEXITCODE -ne 0) { throw "Smoke-Test fehlgeschlagen" }
    }
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    Copy-Item "$root\target\i686-pc-windows-msvc\release\dinput8.dll" $dist -Force
    Copy-Item "$root\target\i686-pc-windows-msvc\release\dinput8.pdb" $dist -Force -ErrorAction SilentlyContinue
    Copy-Item "$root\assets\em4revive.toml" $dist -Force
    Copy-Item "$root\target\i686-pc-windows-msvc\release\EM4Revive.exe" $dist -Force
    Write-Host "Fertig: $dist" -ForegroundColor Green
} finally {
    Pop-Location
}
