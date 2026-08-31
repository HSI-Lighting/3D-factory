# Modify demo — read a part back, then change style, transform, and edit
# geometry through the rasm modify surface.
# Run with:  run modify_demo   (or pyfile scripts/modify_demo.py)

def run():
    # 1) Draw a small part: a bracket plate with a hole.
    rasm.add_layer("parts", set_current=True)
    base = rasm.add_polyline([(0, 0), (40, 0), (40, 20), (0, 20)], closed=True)
    hole = rasm.add_circle((20, 10), 5.0)

    # 2) Read it back — every dict now carries index + full style.
    d = rasm.doc.get(hole)
    print("hole at", d["center"], "radius", d["radius"], "color", d["color"])

    # 3) Style: color the plate, move the hole to a named layer.
    rasm.set_color([base, hole], 5)                 # ACI 5 = blue
    rasm.add_layer("holes", set_current=False)
    rasm.set_layer_of([hole], "holes")
    rasm.set_color([hole], 1)
    rasm.set_lineweight([hole], 0.35)
    print("after style:", rasm.doc.get(hole)["color"],
          rasm.doc.get(hole)["lineweight"], "mm")

    # 4) Transforms: shift the whole part, rotate the hole, shrink the plate.
    rasm.move([base, hole], 50.0, 0.0)
    rasm.rotate([hole], (70.0, 10.0), 45.0)         # around the plate centre
    rasm.scale([base], (50.0, 0.0), 0.5)            # half-size plate

    # 5) Geometry edits: the hole is too small — grow it; plate thickens.
    e = rasm.doc.get(hole)
    e["radius"] = 8.0
    rasm.set_geom(hole, e)
    e = rasm.doc.get(hole)
    e["center"] = (70.0, 10.0)                      # re-centre after the move
    e["radius"] = 6.0
    rasm.set_geom(hole, e)

    # 6) Verify: read the state back and resolve handles after all the edits.
    h = rasm.doc.get(hole)["handle"]
    i = next(i for i, ent in enumerate(rasm.doc.entities())
             if ent["handle"] == h)
    print("hole now at", rasm.doc.get(i)["center"],
          "radius", rasm.doc.get(i)["radius"])

    # 7) Copy the hole for a second part.
    dup = rasm.copy([hole], 0.0, 0.0)
    print("duplicated hole ->", dup, "total entities:", rasm.doc.count())

    # A bad edit fails loudly and leaves everything untouched:
    try:
        e = rasm.doc.get(hole)
        e["type"] = "wall"                          # unsupported geometry type
        rasm.set_geom(hole, e)
    except (RuntimeError, ValueError) as exc:
        print("expected rejection:", exc)

rasm.main(run)
