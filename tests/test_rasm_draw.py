"""rasm add_* API: parameter grids, exact json read-back, style inheritance,
error paths, and set_geom."""

import json

import pytest

from tests.cadcli import check_obj, jrun


def default_layer_color(cadcli):
    """(layer name, entity color string) from a fresh doc."""
    _, (layers, active) = jrun(cadcli, [],
                               "rasm.doc.layers()", "rasm.doc.active_layer()")
    layer = next(l for l in layers if l["id"] == active)
    return layer["name"], layer["color"]


# -- add_* grids ------------------------------------------------------------------

def test_add_line_json(cadcli):
    res, (d,) = jrun(cadcli, ["py rasm.add_line((0,0), (10,5))"],
                     "rasm.doc.get(0)")
    assert len(res.dobjects) == 1
    assert d["type"] == "line"
    assert d["start"] == [0.0, 0.0]
    assert d["end"] == [10.0, 5.0]


def test_add_circle_json(cadcli):
    _, (d,) = jrun(cadcli, ["py rasm.add_circle((-2.5, 3.25), 0.75)"],
                   "rasm.doc.get(0)")
    assert d["type"] == "circle"
    assert d["center"] == [-2.5, 3.25]
    assert d["radius"] == 0.75


def test_add_arc_json(cadcli):
    _, (d,) = jrun(cadcli, ["py rasm.add_arc((1, 2), 4, 350, 20)"],
                   "rasm.doc.get(0)")
    assert d["type"] == "arc"
    assert d["center"] == [1.0, 2.0]
    assert d["radius"] == 4.0
    assert d["start_deg"] == 350.0
    assert d["sweep_deg"] == 20.0


def test_add_ellipse_json(cadcli):
    _, (d,) = jrun(cadcli, ["py rasm.add_ellipse((0, 0), (6, 0), 0.25)"],
                   "rasm.doc.get(0)")
    assert d["type"] == "ellipse"
    assert d["center"] == [0.0, 0.0]
    assert d["major"] == [6.0, 0.0]
    assert d["ratio"] == 0.25


def test_add_polyline_json(cadcli):
    _, (d,) = jrun(cadcli,
                   ["py rasm.add_polyline([(0,0),(3,4),(6,0)], closed=True)"],
                   "rasm.doc.get(0)")
    assert d["type"] == "polyline"
    assert d["points"] == [[0.0, 0.0], [3.0, 4.0], [6.0, 0.0]]
    assert d["closed"] is True


def test_add_point_json(cadcli):
    _, (d,) = jrun(cadcli, ["py rasm.add_point((-7, 8))"], "rasm.doc.get(0)")
    assert d["type"] == "point"
    assert d["at"] == [-7.0, 8.0]


@pytest.mark.parametrize("kwargs,exp", [
    ("", 2.5),
    ("height=1.0", 1.0),
    ("height=0.25", 0.25),
    ("height=12.75, angle_deg=33.0", 12.75),
])
def test_add_text_grid(cadcli, kwargs, exp):
    code = f"rasm.add_text('Hi', (1,2), {kwargs})" if kwargs else \
        "rasm.add_text('Hi', (1,2))"
    _, (d,) = jrun(cadcli, [f"py {code}"], "rasm.doc.get(0)")
    assert d["type"] == "text"
    assert d["text"] == "Hi"
    assert d["at"] == [1.0, 2.0]
    assert d["height"] == exp


def test_add_text_angle_json(cadcli):
    _, (d,) = jrun(cadcli, ["py rasm.add_text('x', (0,0), 2.5, 45.0)"],
                   "rasm.doc.get(0)")
    assert d["angle_deg"] == 45.0


def test_add_returns_sequential_indices(cadcli):
    res = cadcli.run([
        "py print(rasm.add_line((0,0),(1,1)))",
        "py print(rasm.add_circle((0,0), 1))",
        "py print(rasm.add_point((2,2)))",
    ]).expect_ok()
    got = [r for r in res.replies if r in ("0", "1", "2")]
    assert got == ["0", "1", "2"]


def test_new_entities_style_inheritance(cadcli):
    layer_name, layer_color = default_layer_color(cadcli)
    _, (d,) = jrun(cadcli, ["py rasm.add_line((0,0),(1,1))"],
                   "rasm.doc.get(0)")
    assert d["layer"] == layer_name
    assert d["color"] == f"aci {layer_color[4:-1]}"
    assert d["linetype"] == "Continuous"
    assert d["lineweight"] == 0.25
    assert d["visible"] is True
    assert d["handle"] > 0


# -- error paths -------------------------------------------------------------------

@pytest.mark.parametrize("code,needle", [
    ("rasm.add_circle((0,0), 0)", "ValueError: radius must be > 0"),
    ("rasm.add_circle((0,0), -5)", "ValueError: radius must be > 0"),
    ("rasm.add_arc((0,0), 0, 0, 90)", "ValueError: radius must be > 0"),
    ("rasm.add_arc((0,0), -1, 0, 90)", "ValueError: radius must be > 0"),
    ("rasm.add_polyline([(0,0)], closed=False)",
     "ValueError: a polyline needs at least 2 points"),
    ("rasm.add_polyline([], closed=False)",
     "ValueError: a polyline needs at least 2 points"),
    ("rasm.add_text('', (0,0))", "ValueError: text cannot be empty"),
    ("rasm.add_ellipse((0,0), (0,0), 0.5)",
     "ValueError: major axis must be non-zero"),
])
def test_add_error_paths(cadcli, code, needle):
    res = cadcli.run([f"py {code}"]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(needle)
    assert res.dobjects == []


# -- set_geom ----------------------------------------------------------------------

SETUP = """
rasm.add_line((0,0), (10,0))          # 0
rasm.add_circle((5,5), 2)             # 1
rasm.add_arc((0,0), 5, 0, 90)         # 2
rasm.add_ellipse((0,0), (5,0), 0.4)   # 3
rasm.add_polyline([(0,0),(4,0),(4,4)], closed=True)  # 4
rasm.add_point((1,1))                 # 5
rasm.add_text("hello", (2,3))         # 6
print("READY")
"""


def run_setup(cadcli):
    res = cadcli.py_file(SETUP)
    res.expect_ok()
    assert any(r == "READY" for r in res.replies)


@pytest.mark.parametrize("idx,edit,expect_type", [
    (0, "e['start']=(1,1); e['end']=(9,9)", "line"),
    (1, "e['radius']=7.5", "circle"),
    (2, "e['radius']=3.0; e['sweep_deg']=180.0", "arc"),
    (3, "e['major']=(0,6); e['ratio']=0.5", "ellipse"),
    (4, "e['points']=[(0,0),(2,2)]; e['closed']=False", "polyline"),
    (5, "e['at']=(-3,-3)", "point"),
    (6, "e['text']='world'; e['at']=(9,9); e['height']=1.5; e['angle_deg']=30.0",
     "text"),
])
def test_set_geom_every_type(cadcli, idx, edit, expect_type):
    script = SETUP + f"""
import json
before = rasm.doc.get({idx})
e = rasm.doc.get({idx})
{edit}
rasm.set_geom({idx}, e)
after = rasm.doc.get({idx})
assert after["type"] == "{expect_type}", "type changed"
assert after["handle"] == before["handle"], "handle changed"
assert after["layer"] == before["layer"], "layer changed"
assert after["color"] == before["color"], "color changed"
assert after != before, "geometry unchanged"
print("GEOM-OK")
"""
    res = cadcli.py_file(script)
    res.expect_ok()
    assert any(r == "GEOM-OK" for r in res.replies)


def test_set_geom_out_of_range(cadcli):
    run_setup(cadcli)
    res = cadcli.py_file(SETUP + """
e = rasm.doc.get(0)
rasm.set_geom(99, e)
""")
    res.expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no dobject #99")


def test_set_geom_unknown_type(cadcli):
    res = cadcli.py_file(SETUP + """
e = rasm.doc.get(0)
e["type"] = "wall"
rasm.set_geom(0, e)
""")
    res.expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "cannot replace a 'wall' geometry — supported: line, circle, arc, "
        "ellipse, polyline, point, text")


def test_set_geom_missing_field(cadcli):
    res = cadcli.py_file(SETUP + """
rasm.set_geom(0, {"type": "line", "start": (0, 0)})
""")
    res.expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("missing 'end'")


def test_set_geom_polyline_too_few_points(cadcli):
    res = cadcli.py_file(SETUP + """
e = rasm.doc.get(4)
e["points"] = [(0, 0)]
rasm.set_geom(4, e)
""")
    res.expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("a polyline needs at least 2 points")


def test_set_geom_changes_scene_dump(cadcli):
    res = cadcli.py_file(SETUP + """
e = rasm.doc.get(1)
e["radius"] = 9.0
rasm.set_geom(1, e)
print(rasm.command("list"))
""")
    res.expect_ok()
    assert any("circle c=(5.0000,5.0000) r=9.0000" in r for r in res.replies)
