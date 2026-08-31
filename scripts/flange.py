# Parametric pipe flange — a real part drawn from NAMED, TYPED inputs.
#
# Run with:   run flange                                  (opens the input dialog in the app)
#             run flange outer_d=150 bore_d=70 bolts=10   (named inputs, any subset)
#             run flange 150 70 10                        (legacy positional form)
#             pyfile scripts/flange.py                    (all defaults)
#
# The signature + docstring below ARE the declaration: types come from the
# defaults, help text and ranges from the docstring lines
# (`name: help` / `name: help (min..max)`). The dialog and `run k=v …`
# validate/converted through rasm.main.


def run(outer_d: 'length' = 120.0, bore_d: 'length' = 60.0, bolts=6,
        pcd: 'length' = 0.0, hole_d: 'length' = 10.0, axes=True,
        pos: 'point' = (0.0, 0.0), holes_color: 'color' = 5):
    """Parametric pipe flange.
    outer_d: outer diameter (10..1000)
    bore_d: bore diameter, must be < outer_d
    bolts: number of bolts (3..48)
    pcd: bolt-circle diameter, 0 = halfway between OD and bore
    hole_d: bolt-hole diameter (1..100)
    axes: draw the two center lines
    pos: flange center (pick it on the canvas in the dialog)
    holes_color: bolt-hole circle color (ACI)
    """
    import math

    pcd = pcd if pcd > 0 else (outer_d + bore_d) / 2.0
    pos_x, pos_y = pos

    # Validate loudly instead of drawing nonsense.
    if outer_d <= 0 or bore_d <= 0 or hole_d <= 0:
        raise SystemExit("! flange: diameters must be positive")
    if bore_d >= outer_d:
        raise SystemExit("! flange: bore_d must be smaller than outer_d")
    if bolts < 3:
        raise SystemExit("! flange: at least 3 bolts")
    if pcd + hole_d > outer_d - 6:
        raise SystemExit("! flange: pcd + hole_d would break the outer rim")

    print("flange: OD %.1f  bore %.1f  %d bolts on PCD %.1f (hole %.1f)%s at (%.1f, %.1f)"
          % (outer_d, bore_d, bolts, pcd, hole_d,
             "" if axes else ", no axes", pos_x, pos_y))

    # The holes layer — reuse it when it already exists (scripts may run
    # more than once).
    layers = {l["name"]: l for l in rasm.doc.layers()}
    if "flange_holes" not in layers:
        rasm.add_layer("flange_holes", set_current=False)
    rasm.layer_set("flange_holes", color=int(holes_color))
    active_name = [l["name"] for l in rasm.doc.layers()
                   if l["id"] == rasm.doc.active_layer()][0]

    # 1) the body: outer rim + bore (on the current layer)
    rasm.add_circle((pos_x, pos_y), outer_d / 2.0)
    rasm.add_circle((pos_x, pos_y), bore_d / 2.0)

    # 2) bolt holes evenly spaced on the pitch circle
    rasm.set_layer("flange_holes")
    for k in range(bolts):
        a = math.radians(k * 360.0 / bolts)
        rasm.add_circle(
            (pos_x + math.cos(a) * pcd / 2.0, pos_y + math.sin(a) * pcd / 2.0),
            hole_d / 2.0)
    rasm.set_layer(active_name)

    # 3) center marks
    if axes:
        rasm.add_line((pos_x - outer_d / 2.0 - 10, pos_y),
                      (pos_x + outer_d / 2.0 + 10, pos_y))
        rasm.add_line((pos_x, pos_y - outer_d / 2.0 - 10),
                      (pos_x, pos_y + outer_d / 2.0 + 10))

    n = bolts + 2 + (2 if axes else 0)
    print("drew %d entities" % n)
    rasm.set_view((pos_x, pos_y), 500.0 / outer_d)   # fit the part on screen
    print("one Ctrl+Z reverts this whole run")


rasm.main(run)
