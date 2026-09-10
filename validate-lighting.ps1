# Run the SIMLUX-vs-DIALux validation - the 0.50% agreement the product is sold on.
#
# WHY THIS SCRIPT EXISTS. Every validating test in cad_light/tests is [ignore]d behind an external
# fixture directory, so a plain `cargo test` SKIPS them and reports success. The agreement is real
# and nothing in a normal run enforces it. Worse, a missing path is indistinguishable from a pass -
# which is why this refuses loudly rather than skipping.
#
# Run it before AND after any change to cad_light, and before anything from the RUST-AutoRASM 2D
# line is merged in.
#
#   .\validate-lighting.ps1
#   .\validate-lighting.ps1 -Dir "X:\somewhere else"
param(
    [string]$Dir = "D:\Dropbox\YASEEN\3d factory\tests\Identical testing"
)

if (-not (Test-Path (Join-Path $Dir "FONDO.ldt"))) {
    Write-Host "FONDO.ldt not found under: $Dir" -ForegroundColor Red
    Write-Host "A missing fixture reads as a PASS, so this counts as a failure." -ForegroundColor Red
    exit 1
}

$env:IDENTICAL_DIR = $Dir
Write-Host "Validating against DIALux fixtures in: $Dir" -ForegroundColor Cyan

cargo test -p cad_light --test identical_dialux -- --ignored
$rc = $LASTEXITCODE

# The furnished comparison needs a second fixture; run it only when that one exists.
$furniture = Join-Path $Dir "furniture.bin"
if (Test-Path $furniture) {
    $env:IDENTICAL_FURNITURE = $furniture
    cargo test -p cad_light --test identical_dialux_furniture -- --ignored
    if ($LASTEXITCODE -ne 0) { $rc = $LASTEXITCODE }
} else {
    Write-Host "note: furniture.bin absent - the furnished comparison did NOT run." -ForegroundColor Yellow
}

if ($rc -eq 0) { Write-Host "LIGHTING VALIDATION PASSED" -ForegroundColor Green }
else           { Write-Host "LIGHTING VALIDATION FAILED" -ForegroundColor Red }
exit $rc
