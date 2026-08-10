<#
.SYNOPSIS
  Install SIMLUX for the current user. No administrator rights needed.

.DESCRIPTION
  Copies this folder to %LOCALAPPDATA%\Programs\SIMLUX and adds Start Menu and Desktop shortcuts.

  Per-user on purpose. Installing to Program Files needs elevation, and SIMLUX writes nothing
  outside the projects you open, so there is nothing to gain from it.

  The whole folder is copied, not just the exe: the app finds `assets\` beside its binary, and
  without them the door, window, handle and texture libraries are all silently empty.

  Re-running this upgrades in place. Uninstall by deleting the folder and the two shortcuts --
  it touches nothing else, and writes no registry keys.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\SIMLUX'),
    [switch]$NoShortcuts
)

$ErrorActionPreference = 'Stop'
$src = $PSScriptRoot

if (-not (Test-Path (Join-Path $src 'simlux.exe'))) {
    throw "simlux.exe is not next to this script -- run Install.ps1 from inside the unzipped package."
}

Write-Host "Installing SIMLUX to $InstallDir" -ForegroundColor Cyan

# Refuse to install onto a running copy: on Windows the copy would half-succeed, leaving a mix of
# old and new files that behaves like neither.
$running = Get-Process simlux -ErrorAction SilentlyContinue
if ($running) {
    throw "SIMLUX is running (PID $($running.Id -join ', ')). Close it and run this again."
}

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

# Copy everything except this script and the readme.
Get-ChildItem -Path $src -Exclude 'Install.ps1', 'README.txt' | ForEach-Object {
    Copy-Item -Recurse -Force $_.FullName -Destination $InstallDir
}

$exe = Join-Path $InstallDir 'simlux.exe'
if (-not (Test-Path $exe)) { throw "copy failed -- $exe is not there" }

if (-not $NoShortcuts) {
    $ws = New-Object -ComObject WScript.Shell
    $targets = @(
        (Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\SIMLUX.lnk'),
        (Join-Path ([Environment]::GetFolderPath('Desktop'))   'SIMLUX.lnk')
    )
    foreach ($lnk in $targets) {
        $dir = Split-Path -Parent $lnk
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        $s = $ws.CreateShortcut($lnk)
        $s.TargetPath = $exe
        # WorkingDirectory is set for tidiness only. The app resolves its assets from the
        # executable's own location, precisely so a shortcut launched from anywhere still works.
        $s.WorkingDirectory = $InstallDir
        $s.Description = 'SIMLUX -- Lighting Designer'
        $s.Save()
        Write-Host "  shortcut: $lnk"
    }
}

Write-Host ""
Write-Host "Installed." -ForegroundColor Green
Write-Host "  $exe"
Write-Host ""
Write-Host "To uninstall: delete $InstallDir and the two SIMLUX shortcuts. Nothing else is touched."
