<#
.SYNOPSIS
  Build SIMLUX and assemble a self-contained package for installing on another PC.

.DESCRIPTION
  Produces  dist\SIMLUX-Build-<n>\  containing the binary, every runtime asset, Install.ps1 and a
  BUILD-INFO.txt naming the commit -- then zips it. Nothing else is needed on the target machine:
  the app is a single native executable and its assets sit beside it.

  The build number lives in packaging\build-number.txt and only moves when -NewBuild is passed.

  The asset copy is the part that matters. The app resolves `assets/...` relative to the EXECUTABLE
  (see cad_app/src/assets.rs), so the layout here is not cosmetic -- it is the contract. Ship the
  exe without `assets/` and the door, window, handle and CC0 texture libraries are all silently
  empty, because each of those treats a missing folder as "nothing to offer" rather than an error.

.PARAMETER SkipBuild
  Package whatever is already in target\release. Useful when the build was just run.

.PARAMETER NewBuild
  Advance packaging\build-number.txt before packaging, i.e. cut the NEXT build.
  Without it the current number is re-used, so re-packaging the same code does not burn a number.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\build-package.ps1
  powershell -ExecutionPolicy Bypass -File packaging\build-package.ps1 -NewBuild
#>
[CmdletBinding()]
param([switch]$SkipBuild, [switch]$NewBuild)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# The RELEASE NUMBER people say out loud -- "build 3". A short commit hash is precise and
# impossible to remember or repeat over a phone; a sequence is the opposite. Both are kept: the
# folder carries the number, and BUILD-INFO.txt inside it carries the commit, so a report of
# "build 3 is doing X" still leads straight back to the source it was cut from.
$numFile = Join-Path $PSScriptRoot 'build-number.txt'
if (-not (Test-Path $numFile)) { Set-Content -Path $numFile -Value '1' -Encoding ascii }
$build = [int](Get-Content $numFile -Raw).Trim()
if ($NewBuild) {
    $build++
    Set-Content -Path $numFile -Value $build -Encoding ascii
    Write-Host "Cutting build $build (build-number.txt advanced)" -ForegroundColor Cyan
    # The number is compiled INTO the binary, so a bumped number needs a rebuild to take effect.
    if ($SkipBuild) { Write-Warning "-SkipBuild with -NewBuild: the exe will still report the OLD number" }
}

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

$name = "SIMLUX-Build-$build"
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

# What the folder name no longer says. Someone reporting "build 3 does X" is answerable from here
# without them having to read a hash off a screen.
$info = @"
SIMLUX  --  Build $build

  commit    $commit
  packaged  $(Get-Date -Format 'yyyy-MM-dd HH:mm')

The running app states the same pair on its first line of output, and the `scene` command repeats
it at the top of every report -- so a dump can always be tied back to the exact source.
"@
Set-Content -Path (Join-Path $out 'BUILD-INFO.txt') -Value $info -Encoding ascii

# A quick integrity check, so a broken package is caught HERE and not on the other machine.
$missing = @()
foreach ($f in @('simlux.exe', 'assets\apertures\window.obj', 'assets\handles\handles.json', 'Install.ps1', 'BUILD-INFO.txt')) {
    if (-not (Test-Path (Join-Path $out $f))) { $missing += $f }
}
if ($missing.Count) { throw "package is incomplete, missing: $($missing -join ', ')" }

$zip = Join-Path $repo "dist\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$out\*" -DestinationPath $zip

$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host ""
Write-Host "Build $build ready:" -ForegroundColor Green
Write-Host "  $zip  ($mb MB)"
Write-Host "  commit $commit"
Write-Host ""
Write-Host "On the other PC: unzip, then right-click Install.ps1 -> Run with PowerShell"
Write-Host "(or just run simlux.exe from the unzipped folder -- it is portable)"
