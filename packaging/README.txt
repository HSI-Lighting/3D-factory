SIMLUX -- Lighting Designer
==========================

WHAT IS NEW IN THIS BUILD
-------------------------
Two of these change numbers you may already have quoted. Read those first.

  * ILLUMINANCE IS NOW MAINTAINED, NOT INITIAL.
    Every lux figure carries an EN 12464 maintenance factor (default 0.80, editable as its four
    CIE 97 sub-factors under "Calculation"). Results used to be the day-one condition presented as
    the answer, which overstates a design by the whole of that factor. The panel now says which
    condition it is quoting. A project saved before this reopens at its original condition rather
    than being silently restated.

  * INTERREFLECTION WAS UNDER-READING BY ABOUT A QUARTER.
    The bounce solver cost rays^bounces, so it shipped at one bounce -- which reports roughly three
    quarters of the light actually in a room. It is now linear in bounces and defaults to five.
    Together with maintenance the two corrections partly cancel, so existing rooms will not move
    dramatically, but both numbers are now right for the right reasons.

  * 3D FACTORY: CHOOSE THE UNIT YOU BUILD IN.
    First control on the Factory toolbar -- mm, cm, m, in, ft -- defaulting to MILLIMETRES.
    Geometry is always stored in metres, so switching the unit never moves anything.
    An imported drawing whose file declares no unit (most DXFs omit it) is now read at that working
    unit instead of being assumed to be metres. That assumption was turning a 4400 mm outline into
    a 4400 METRE one -- a building 4.4 km across and three metres tall, which draws as a flat sheet.

  * LIGHT POINTS YOU CAN MOVE.
    Place points by clicking the plan, then choose which imported fitting goes in them. Drag to
    move, Shift-click to extend the selection, Del to remove. Fittings import through the same file
    browser as furniture (IES and EULUMDAT). The layout is saved with the project -- it was not
    before.

  * MORE OF WHAT A REPORT NEEDS.
    Vertical, cylindrical, semi-cylindrical and scalar illuminance; surface luminance; U1, median
    and percentiles; direct/indirect split; connected load, W/m2 and lm/W. Mean cylindrical
    illuminance at eye height is reported with its EN 12464 verdict.

  * A EULUMDAT READING ERROR IS FIXED.
    Multi-lamp-set .ldt files were being SUMMED. The sets are alternative lamp configurations, not
    simultaneous lamps -- so a three-set file over-lit by 2.75x. Caught by comparing against a
    DIALux report on a real project.

Known limitation: nothing here validates a fitting's own data. A .ldt claiming 1000 lm/W will be
used as written, by this and by every other tool. If a result looks impossible, check the file.


INSTALL
-------
Right-click Install.ps1 and choose "Run with PowerShell".

That copies this folder to %LOCALAPPDATA%\Programs\SIMLUX and adds Start Menu and Desktop
shortcuts. No administrator rights are needed and nothing is written to the registry.

If Windows blocks the script, open PowerShell in this folder and run:

    powershell -ExecutionPolicy Bypass -File .\Install.ps1

You can also just run simlux.exe from this folder without installing -- it is fully portable.
What you cannot do is move simlux.exe on its own: it loads the doors, windows, handles and
texture libraries from the assets\ folder beside it, and without them those libraries are
silently empty.


REQUIREMENTS
------------
  * Windows 10 or 11, 64-bit
  * A GPU with OpenGL 3.3 (anything from the last decade)
  * ~8 GB RAM for large projects; the gym model uses about 3 GB

No runtime to install -- the executable is native and self-contained.


UNINSTALL
---------
Delete %LOCALAPPDATA%\Programs\SIMLUX and the two SIMLUX shortcuts. That is all of it.


TROUBLESHOOTING
---------------
Black window, or it closes at once
    The GPU driver is too old for OpenGL 3.3, or you are on a remote desktop session without
    hardware acceleration. Update the display driver.

Doors, windows or textures are missing from the menus
    simlux.exe has been separated from its assets\ folder. Keep them together.

Which build is this?
    Type `scene` in the command line -- the first line of the report names the commit.

Something renders or cuts wrongly
    Type `scene` (whole-model state) or `diag` (geometry and opening checks) and send the output.
    Both print to the panel and can be copied straight out of it. That is far more use than a
    screenshot: it carries the coordinates, materials, cut depths and render settings.
