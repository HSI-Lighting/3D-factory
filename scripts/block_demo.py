# Blocks demo — select geometry, define a block, stamp instances.
# Run with:  pyfile scripts/block_demo.py
#
# create_block() consumes the current selection (like the `block` command).

# 1) draw the unit (a square with a diagonal)
sq = []
sq.append(rasm.add_line((0, 0), (10, 0)))
sq.append(rasm.add_line((10, 0), (10, 10)))
sq.append(rasm.add_line((10, 10), (0, 10)))
sq.append(rasm.add_line((0, 10), (0, 0)))
sq.append(rasm.add_line((0, 0), (10, 10)))

# 2) select it, define the block at its corner, selection is consumed
rasm.set_selection(sq)
rasm.create_block("unit_square", (0, 0))
print("blocks:", rasm.doc.blocks())

# 3) stamp three instances
for i, at in enumerate([(30, 0), (60, 0), (30, 30)]):
    rasm.insert_block("unit_square", at, rotation_deg=0.0)
print("doc now has", rasm.doc.count(), "entities")
rasm.set_view((30, 15), 12.0)
