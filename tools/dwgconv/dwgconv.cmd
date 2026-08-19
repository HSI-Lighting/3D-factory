@echo off
setlocal enabledelayedexpansion
rem =====================================================================
rem  dwgconv - DWG to DXF, for SIMLUX.
rem
rem  Usage:  dwgconv.cmd  <input.dwg>  <output.dxf>
rem
rem  SIMLUX reads DXF and not DWG, and it never will directly: DWG is a
rem  closed, versioned, undocumented format. Every CAD package that opens
rem  one either licenses Autodesk's RealDWG or the Open Design Alliance's
rem  library. So the app shells out, and this is the shell-out.
rem
rem  WHAT DOES THE WORK is AutoCAD's own headless core, accoreconsole.exe,
rem  which ships with any AutoCAD install and is exactly as correct as
rem  AutoCAD is. If AutoCAD is not installed, set RUSTCAD_DWGCONV to your
rem  own converter instead - "ODAFileConverter" and Teigha's dwg2dxf both
rem  fit, and the app's template form takes {in} and {out}.
rem
rem  THE SCRIPT IS WRITTEN PER RUN, into the output file's folder, because
rem  the DXF filename has to be baked into it - accoreconsole takes a
rem  script, not arguments, and answers its prompts from that script's
rem  lines in order.
rem
rem  FILEDIA 0 first. With the file dialogs enabled, DXFOUT opens a Save
rem  As window that nothing is there to click, and the run hangs until it
rem  is killed - which reads as "the converter is slow", not as "it is
rem  waiting for a person".
rem
rem  DXFOUT, NOT -DXFOUT. There is no dashed form of this command; asking
rem  for one gets 'Unknown command "-DXFOUT"' and an exit code of zero,
rem  so the only symptom is a DXF that never appears.
rem =====================================================================

set "IN=%~1"
set "OUT=%~2"
if "%IN%"=="" goto :usage
if "%OUT%"=="" goto :usage
if not exist "%IN%" (
  echo dwgconv: no such file: %IN% 1>&2
  exit /b 2
)

rem ---- find accoreconsole ----
set "ACC="
if defined RUSTCAD_ACCORECONSOLE if exist "%RUSTCAD_ACCORECONSOLE%" set "ACC=%RUSTCAD_ACCORECONSOLE%"
if not defined ACC (
  for /f "delims=" %%P in ('dir /b /s "C:\Program Files\Autodesk\accoreconsole.exe" 2^>nul') do (
    if not defined ACC set "ACC=%%P"
  )
)
if not defined ACC (
  echo dwgconv: accoreconsole.exe not found under C:\Program Files\Autodesk. 1>&2
  echo          Set RUSTCAD_ACCORECONSOLE to it, or set RUSTCAD_DWGCONV to 1>&2
  echo          another converter as "cmd {in} {out}". 1>&2
  exit /b 3
)

rem ---- write the script beside the output ----
for %%F in ("%OUT%") do set "OUTDIR=%%~dpF"
set "SCR=%OUTDIR%dwgconv_%RANDOM%%RANDOM%.scr"
> "%SCR%" (
  echo FILEDIA
  echo 0
  echo DXFOUT
  echo %OUT%
  echo 16
  echo QUIT
  echo Y
)

if exist "%OUT%" del /q "%OUT%"
"%ACC%" /i "%IN%" /s "%SCR%" >nul 2>&1
del /q "%SCR%" 2>nul

if not exist "%OUT%" (
  echo dwgconv: accoreconsole produced no DXF from "%IN%". 1>&2
  echo          Run it by hand to see why - a drawing that needs a 1>&2
  echo          missing SHX font or xref can stop at a prompt. 1>&2
  exit /b 4
)
exit /b 0

:usage
echo usage: dwgconv.cmd ^<input.dwg^> ^<output.dxf^> 1>&2
exit /b 1
