@echo off
setlocal enabledelayedexpansion
rem =====================================================================
rem  dxf2dwg - DXF to DWG, for SIMLUX.
rem
rem  Usage:  dxf2dwg.cmd  <input.dxf>  <output.dwg>
rem
rem  THE MIRROR OF dwgconv.cmd, and for the same reason: DWG is a closed,
rem  versioned, undocumented format, so the app writes a DXF and asks
rem  AutoCAD's own headless core to save it as a DWG. Anything that can
rem  open a DWG can already open the DXF we write; this exists because a
rem  practice's filing, its consultants and its clients ask for .dwg.
rem
rem  SAVEAS 2018, NOT QSAVE. accoreconsole opened on a DXF has no DWG
rem  name to save back to, so QSAVE writes the DXF again and reports
rem  success. SAVEAS names the file and the format in one go.
rem
rem  FILEDIA 0 FIRST, exactly as the other direction needs it: with the
rem  dialogs enabled, SAVEAS opens a window nothing is there to click and
rem  the run hangs until it is killed - which reads as "the converter is
rem  slow", not as "it is waiting for a person".
rem
rem  THE VERSION IS PINNED. "2018" is the last format Autodesk changed and
rem  is read by every release since; leaving it to the default means the
rem  file silently follows whichever AutoCAD happens to be installed, so
rem  two machines in one office produce drawings the other cannot open.
rem =====================================================================

set "IN=%~1"
set "OUT=%~2"
if "%IN%"=="" goto :usage
if "%OUT%"=="" goto :usage
if not exist "%IN%" (
  echo dxf2dwg: no such file: %IN% 1>&2
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
  echo dxf2dwg: accoreconsole.exe not found under C:\Program Files\Autodesk. 1>&2
  echo          Set RUSTCAD_ACCORECONSOLE to it, or set RUSTCAD_DXF2DWG to 1>&2
  echo          another converter as "cmd {in} {out}". 1>&2
  exit /b 3
)

rem ---- write the script beside the output ----
for %%F in ("%OUT%") do set "OUTDIR=%%~dpF"
set "SCR=%OUTDIR%dxf2dwg_%RANDOM%%RANDOM%.scr"
> "%SCR%" (
  echo FILEDIA
  echo 0
  echo SAVEAS
  echo 2018
  echo %OUT%
  echo QUIT
  echo Y
)

if exist "%OUT%" del /q "%OUT%"
"%ACC%" /i "%IN%" /s "%SCR%" >nul 2>&1
del /q "%SCR%" 2>nul

if not exist "%OUT%" (
  echo dxf2dwg: accoreconsole produced no DWG from "%IN%". 1>&2
  echo          Run it by hand to see why - a drawing that needs a 1>&2
  echo          missing SHX font can stop at a prompt. 1>&2
  exit /b 4
)
exit /b 0

:usage
echo usage: dxf2dwg.cmd ^<input.dxf^> ^<output.dwg^> 1>&2
exit /b 1
