<#
.SYNOPSIS
  Build SIMLUX and assemble a self-contained package for installing on another PC.

.DESCRIPTION
  Produces  dist\SIMLUX-<commit>\  containing the binary, every runtime asset, and Install.ps1 --
  then zips it. Nothing else is needed on the target machine: the app is a single native
  executable and its assets sit beside it.

  The asset copy is the part that matters. The app resolves `assets/...` relative to the EXECUTABLE
  (see cad_app/src/assets.rs), so the layout here is not cosmetic -- it is the contract. Ship the
  exe without `assets/` and the door, window, handle and CC0 texture libraries are all silently
  empty, because each of those treats a missing folder as "nothing to offer" rather than an error.

.PARAMETER SkipBuild
  Package whatever is already in target\release. Useful when the build was just run.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\build-package.ps1
#>
[CmdletBinding()]
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# Name the package after the commit it was built from, so two packages are never confused for
# each other -- this project has already lost hours to "which build is this?".
$commit = (& git rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = 'unknown' }
$dirty = (& git status --porcelain 2>$null)
if ($dirty) { $commit = "$commit+dirty" }

if (-not $SkipBuild) {
    Write-Host "Building release ($commit)..." -ForegroundColor Cyan
    & cargo build --release -p cad_app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$exe = Join-Path $repo 'target\release\simlux.exe'
if (-not (Test-Path $exe)) { throw "simlux.exe not found -- run without -SkipBuild" }

$name = "SIMLUX-$commit"
$out  = Join-Path $repo "dist\$name"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force -Path $out | Out-Null

Write-Host "Assembling $out" -ForegroundColor Cyan
Copy-Item $exe (Join-Path $out 'simlux.exe')

# EVERY runtime asset root. `assets\test` is fixtures for the test suite and is deliberately not
# shipped; everything else is loaded by the running app.
foreach ($d in @('apertures', 'cc0', 'handles')) {
    $src = Join-Path $repo "assets\$d"
    if (Test-Path $src) {
        Copy-Item -Recurse $src (Join-Path $out "assets\$d")
    } else {
        Write-Warning "assets\$d is missing -- the matching library will be empty in the app"
    }
}
# The logo is looked for beside the binary too.
$logo = Join-Path $repo 'cad_app\assets\logo.svg'
if (Test-Path $logo) {
    New-Item -ItemType Directory -Force -Path (Join-Path $out 'assets') | Out-Null
    Copy-Item $logo (Join-Path $out 'assets\logo.svg')
}

Copy-Item (Join-Path $PSScriptRoot 'Install.ps1') $out
Copy-Item (Join-Path $PSScriptRoot 'README.txt')  $out

# A quick integrity check, so a broken package is caught HERE and not on the other machine.
$missing = @()
foreach ($f in @('simlux.exe', 'assets\apertures\window.obj', 'assets\handles\handles.json', 'Install.ps1')) {
    if (-not (Test-Path (Join-Path $out $f))) { $missing += $f }
}
if ($missing.Count) { throw "package is incomplete, missing: $($missing -join ', ')" }

$zip = Join-Path $repo "dist\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$out\*" -DestinationPath $zip

$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host ""
Write-Host "Package ready:" -ForegroundColor Green
Write-Host "  $zip  ($mb MB)"
Write-Host ""
Write-Host "On the other PC: unzip, then right-click Install.ps1 -> Run with PowerShell"
Write-Host "(or just run simlux.exe from the unzipped folder -- it is portable)"
