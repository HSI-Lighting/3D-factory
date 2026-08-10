SIMLUX -- Lighting Designer
==========================

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
