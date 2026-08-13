SIMLUX -- Lighting Designer
==========================

WHAT IS NEW IN BUILD 4
----------------------
Everything in this section is 3D Factory. The lighting engine is unchanged since build 3, whose
notes follow below and still apply -- including the two that changed numbers you may have quoted.

  * ROOMS ARE OBJECTS.
    A Rooms menu lists them; each can be renamed, re-heighted, and given its own floor, ceiling and
    wall thicknesses. Names appear on the plan. Openings are grouped under the room they are in.
    Deleting a room fills the hole back in, so the space can be rebuilt. Room height is the CLEAR
    height -- floor to ceiling -- and the app now says so where you type it.

  * YOU CHOOSE WHERE A NEW OBJECT LANDS.
    Everything used to appear at one fixed point with no say in it. Type `place` in the 3D command
    line for:  Placement [Click/Centre/Origin/Offset] <click>:
    "Click" is the default -- the object waits for a click in EITHER window before it settles, and
    Esc leaves it where it is. "Offset" then asks for the distance:  X,Y,Z <200 mm, 0, 0>:  and
    takes it with or without a leading @. Select something and choose "click" to re-place a piece
    that is already built. Applies to 3D solids, furniture, apertures and architecture; a door or
    window drawn on a wall is unaffected, since it already goes where you drew it.

  * ONE COMMAND LINE, TWO WINDOWS.
    Its title reads "3D Command" or "Command" depending on which window you last clicked, and it
    says which unit new work is in and where objects will land. A 2D drawing command typed at the
    3D prompt is refused with the reason, and vice versa -- click the window you want.

  * THE 2D VIEWS ARE THE FACES YOU DREW ON.
    Top/Front/Back/Left/Right are gone. The list is now Global view (the plan, showing the whole
    model from above -- solids, rooms and furniture) plus every face you have drawn on. Faces are
    named after the object they belong to, can be renamed to what the drawing is OF, and can be
    deleted. Clicking a face again reopens the same plane with the same drawing on it -- and those
    drawings now SURVIVE A SAVE, which they never did before.

  * DRAW ON A FACE AND SEE THAT FACE.
    The 2D underlay used to project the whole model onto the plane, which on one wall of a large
    building is every wall and opening flattened on top of each other. It is now the face itself.
    The picked face is outlined in yellow in the 3D view so you can see which one you are on.

  * A GROUND GRID THAT IS ACTUALLY THERE.
    It follows the camera and steps its spacing with the zoom, so it covers the view at any scale
    instead of being a 20 m patch at the world origin. GRID3D toggles it, ORG shows the world-origin
    axes, SNAP3D turns 3D corner-snapping on and off, and FURN still shows the 3D model on the plan.

  * THE UNIT IS ASKED FOR ONCE.
    Opening the 3D Factory on a new project asks which unit you are building in, so it cannot be
    missed. Once answered it is not asked again, and the toolbar still changes it at any time.

Known limitation: 3D snapping catches solid corners only -- not furniture -- and its 12-pixel
aperture is measured on screen, so it reaches further in world terms the further you zoom out.
SNAP3D turns it off.


WHAT WAS NEW IN BUILD 3
-----------------------
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
