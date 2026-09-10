# Grid of circles — a classic batch-scripting demo (loop + math + inputs).
# Run with:  run grid_circles          (defaults: 8 x 6, spacing 10)
#            run grid_circles 12 8 15  (12 cols, 8 rows, spacing 15)
#            pyfile scripts/grid_circles.py
#
# Inputs arrive via rasm.args (list of strings) and sys.argv.

def arg(i, default):
    return rasm.args[i] if i < len(rasm.args) else default

cols = int(arg(0, "8"))
rows = int(arg(1, "6"))
spacing = float(arg(2, "10.0"))
r = 2.5

print("drawing a %dx%d grid of circles (spacing %.1f)" % (cols, rows, spacing))
n0 = rasm.doc.count()
for iy in range(rows):
    for ix in range(cols):
        rasm.add_circle(
            ((ix - (cols - 1) / 2.0) * spacing,
             (iy - (rows - 1) / 2.0) * spacing),
            r,
        )
print("added", rasm.doc.count() - n0, "circles")
rasm.set_view((0, 0), 24.0)   # center on the grid, zoomed out to see it
print("view:", rasm.view())
