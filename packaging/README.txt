SIMLUX -- Lighting Designer
==========================

WHAT IS NEW SINCE BUILD 4
-------------------------
READ THE FIRST FOUR ITEMS EVEN IF YOU READ NOTHING ELSE. They change numbers you may already
have quoted, and they change them for good reasons -- but a figure that moves without warning is
worse than one that was wrong quietly.


NUMBERS THAT HAVE MOVED

  * THE LUX GRID IS NOW THE ONE THE PANEL SAYS IT IS.
    The calculation plane was capped at 64 cells PER AXIS. On any room longer than 64 cells that
    coarsened the long side and left the short one alone, so the grid came out RECTANGULAR while
    the cell-size field said one number. A 33 x 13 m hall at the default 0.25 m was being sampled
    at 0.52 m along its length and 0.25 m across -- and average, minimum and uniformity were all
    taken over two resolutions at once.
    Both axes are now coarsened together when a budget is reached, so a cell stays square, and the
    budget is four times larger: measured at 0.03 ms per point on a bare room and 0.08 ms on a
    busy one, so 16,384 points is about half a second. A 33 x 13 m hall now gets the 0.25 m it
    asks for. EXPECT E-min AND U0 TO COME DOWN on rooms bigger than about 16 m: a finer grid finds
    a lower minimum, which is the more honest number and not a worse design.
    When a room IS too large for the budget, the results line now says the spacing that was
    actually used instead of leaving you to assume.

  * UNIFORMITY IS NOW QUOTED TWICE, on the grid you set and on EN 12464-1's.
    U0 is not a property of a room. It is a property of a room AND the grid it was sampled on, and
    the standard specifies its own -- which GROWS with the room: 1.94 m across a 33 m hall, 5 m
    across a 100 m plan. Your working grid at 0.25 m is FINER than that for every room down to
    about 3 m, so the figure this panel has always shown is the conservative one.
    The Report panel now shows both, each labelled with its grid. The EN figure will be the HIGHER
    of the two. Neither replaces the other: one says how even the room really is, the other is what
    a compliance claim rests on.

  * A ROOM TRACED WITH A SPLINE IS NOW LIT.
    Splines were silently dropped by the lighting engine, so a curved wall drafted with one was
    calculated as though it were not there -- open air, no error, a plausible-looking lux figure.
    Any project with a spline-traced room will now report DIFFERENT and correct numbers.

  * EVERY PROJECT REOPENS WITH FEWER TRIANGLES. THE SOLIDS ARE UNCHANGED.
    The CSG library's bounding-box optimisation is now on. It feeds only the polygons that can
    actually overlap into each boolean and passes the rest through whole, so it legitimately
    produces fewer triangles for the SAME solid -- a wall that was split by a tree with no business
    splitting it comes back in one piece. Measured on a real 172-feature project: total evaluation
    1,759 ms to 165 ms, and triangles 10,275 to 5,623.
    If you have a triangle count written down anywhere, it will not match. The shape will.


THE CALCULATION USED TO FREEZE. IT DOES NOT NOW.

    On a real project with furniture in it, pressing Calculate could stop the window responding for
    fifteen minutes or more -- long enough that Windows greys it out and you close it. It was not a
    crash, which is why there was never anything in a log.
    The surface report was taking AT LEAST ONE ray-traced sample PER TRIANGLE, so a 450,000-triangle
    chair covering two square metres took 450,000 measurements instead of two. On a seven-million-
    triangle scene that is seven million. It now samples by AREA, capped.
    Separately, every phase of the calculation was building its own copy of the scene's search tree
    -- four times over, at 1.9 seconds each.

        before   never returned
        after    2.9 seconds, on the same project

    If a calculation ever does hang again, run simlux.exe with SIMLUX_PHASE_LOG=1 and it will print
    each phase as it finishes, so the one that did not come back is named rather than guessed at.


DRAWING AND IMPORT

  * MIRRORED DXF ENTITIES IMPORT THE RIGHT WAY ROUND.
    AutoCAD writes a "-Z extrusion" on a circle, arc or block reference whenever it is mirrored,
    and several exporters write it for whole drawings. It was being ignored, so those entities came
    in back to front -- and only those, so a plan could arrive with half its blocks flipped and the
    walls exactly where they should be. Arcs, polyline bulges and block rotations are all handled.
    Checked against the DXFs in your own test folder: none of them carry a mirrored extrusion, so
    nothing you already have will move.

  * A PLATE WITH A BOLT HOLE IS ONE OBJECT.
    Draw an outline with shapes inside it and Extrude: the inner loops are now HOLES rather than
    separate solids standing in the middle of the first. A washer with a pin in its bore still
    comes out as a washer with a hole AND a pin.

  * A PLANE SHARES THE DRAWING'S LAYERS, STYLES AND BLOCKS -- all of them.
    Layers and blocks already crossed into a face sketch. Linetypes, pens, true colours, text
    styles, dimension styles and wall styles did not, so anything drawn on a plane against those
    meant something else when read back in the plan.

  * THE GRID AND THE CLIPBOARD KNOW WHICH SPACE THEY ARE IN.
    A face sketch is measured in metres; your plan is probably in millimetres. A 10 mm grid became
    a TEN METRE grid inside a sketch, and a 3 m wall copied from the plan pasted into a sketch as a
    three kilometre one. Both convert now. Nothing changes when you are not in a sketch.


3D MODELLING

  * DELETING A BODY NO LONGER MOVES ANOTHER BODY'S HOLES.
    An opening used to be bound to whichever solid happened to sit before it in the feature list,
    so deleting or reordering anything could silently re-home every opening after it. An opening
    now names the wall it is cut in.

  * ESCAPING AN OPENING EDIT PUTS THE OPENING BACK -- in the installed build.
    It always did in the development build. In the shipped one the restore was inside an assertion
    that release builds compile away, so leaving an opening edit without pressing Apply destroyed
    it. If you have lost a window that way, that is why.

  * A PLANE HAS AN IDENTITY. Deleting one no longer renames or corrupts another.
  * A PAINTED FACE KEEPS ITS PAINT when the object is rotated or scaled, not only moved.
  * A CUT REMEMBERS WHETHER IT WAS MEANT TO GO THROUGH, so a recess is no longer reported as a
    failed through-cut. On a real project 6 of 18 warnings were pockets working exactly as drawn.


LIGHT EDITOR (SIMLUX menu)

  * Pair a block in the drawing with a photometric file, and one fitting is placed at every
    instance of it -- the positions are already in the drawing, so this is work you should not be
    doing by hand.
  * "+ Add block" shows every block in the drawing WITH ITS LINEWORK DRAWN, so you can see which
    one is the downlight.
  * "+ Add light" shows the photometry WITH ITS DISTRIBUTION CURVE, flux, power, efficacy, peak
    candela and beam angle. A figure the file does not state is shown as a dash, never as a
    plausible number.
  * The folder picker lists your .ies / .ldt files, so you can see you are in the right folder.


IF SOMETHING GOES WRONG

    A crash now writes simlux-crash.log next to simlux.exe, with the build number and where it
    happened. Send that file. If the app hangs rather than crashes, SIMLUX_PHASE_LOG=1 (above) is
    the one to run.

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
