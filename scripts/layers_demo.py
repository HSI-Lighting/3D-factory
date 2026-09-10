# Layers demo — create layers, set the active one, and place entities.
# Run with:  pyfile scripts/layers_demo.py

wall = rasm.add_layer("walls", set_current=False)   # returns the layer id
door = rasm.add_layer("doors")
print("layers:", rasm.doc.layers())

rasm.set_layer("walls")
rasm.add_line((0, 0), (40, 0))
rasm.add_line((0, 20), (40, 20))
rasm.add_line((0, 0), (0, 20))

rasm.layer_set("walls", color=5)          # ACI 5 = blue
rasm.set_layer("doors")
rasm.add_line((20, 0), (20, 10))
rasm.add_arc((20, 10), 3.0, 0.0, 180.0)

print("active layer is now", rasm.doc.active_layer(), "(doors)")
print("note: Ctrl+Z once reverts this whole run")
