# Hello, RUST-AutoRASM — the smallest possible script.
# Run with:  pyfile scripts/hello.py
#            (or "Run file" in the Python console: `py`)

print("hello from python")
c = rasm.add_circle((0, 0), 5.0)
l = rasm.add_line((0, 0), (5, 5))
print("added circle #", c, "and line #", l)
print("doc now has", rasm.doc.count(), "entities")

e = rasm.doc.get(c)
print("circle snapshot:", e["type"], "radius", e["radius"], "on layer", repr(e["layer"]))
