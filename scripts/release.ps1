# Veröffentlicht eine neue Version: Versionsnummer setzen, committen, Tag pushen.
# GitHub baut daraufhin das Setup und erstellt das Release; installierte Launcher
# holen es sich beim nächsten Start.
#   .\scripts\release.ps1 -Version 0.3.0
param(
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$env:Path += ";$env:USERPROFILE\.cargo\bin"
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version muss so aussehen: 0.3.0" }

Push-Location $root
try {
    if (git status --porcelain) { throw "Es gibt noch nicht committete Änderungen. Bitte erst committen." }
    if (-not (git config --get remote.origin.url)) { throw "Kein GitHub-Remote: git remote add origin https://github.com/NAME/PROJEKT.git" }
    if ((git tag --list "v$Version")) { throw "Tag v$Version existiert bereits." }

    $manifest = Join-Path $root 'Cargo.toml'
    $text = [IO.File]::ReadAllText($manifest)
    $current = [regex]::Match($text, '(?m)^version = "(.+)"').Groups[1].Value
    if ([version]$Version -le [version]$current) { throw "$Version ist nicht neuer als die aktuelle Version $current." }
    $text = ([regex]'(?m)^version = ".+"').Replace($text, "version = `"$Version`"", 1)
    [IO.File]::WriteAllText($manifest, $text)

    cargo update --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo update fehlgeschlagen" }
    git add Cargo.toml Cargo.lock
    git commit -m "Version $Version"
    git tag "v$Version"
    git push origin HEAD "v$Version"
    if ($LASTEXITCODE -ne 0) { throw "git push fehlgeschlagen" }
    Write-Host "v$Version ist unterwegs. Fortschritt: GitHub-Projekt > Actions" -ForegroundColor Green
} finally {
    Pop-Location
}
