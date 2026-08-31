"""rasm.doc read surface + selection/sysvars/view/layout/current-style
headless behaviors. All scenarios batch ops and read-backs into one process."""

import pytest

from tests.cadcli import check_obj, jrun


# -- doc surface -------------------------------------------------------------------

def test_doc_count_empty(cadcli):
    _, (count, ents, bounds) = jrun(cadcli, [],
                                    "rasm.doc.count()",
                                    "rasm.doc.entities()",
                                    "rasm.doc.bounds()")
    assert count == 0
    assert ents == []
    assert bounds is None


def test_doc_count_after_adds(cadcli):
    _, (count, ents) = jrun(cadcli, [
        "line 0,0 1,1", "circle 0,0 2",
    ], "rasm.doc.count()", "rasm.doc.entities()")
    assert count == 2
    assert len(ents) == 2
    assert ents[0]["type"] == "line"
    assert ents[1]["type"] == "circle"


def test_doc_get_snapshot_fields(cadcli):
    _, (d,) = jrun(cadcli, ["line 0,0 10,0"], "rasm.doc.get(0)")
    assert set(d) >= {"handle", "layer", "color", "linetype", "lineweight",
                      "visible", "type", "start", "end"}
    assert d["handle"] > 0


def test_doc_get_out_of_range(cadcli):
    res = cadcli.run(["py rasm.doc.get(0)"]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no dobject #0 (0 total)")
    res = cadcli.run(["line 0,0 1,1",
                      "py rasm.doc.get(5)"]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no dobject #5 (1 total)")


def test_doc_units(cadcli):
    d = cadcli.py_json("rasm.doc.units()")
    assert d["name"] == "mm"
    assert d["scene_per_unit"] == 1.0


def test_doc_bounds_math(cadcli):
    _, (b,) = jrun(cadcli, [
        "line -5,-5 5,5", "circle 0,0 2", "point 100,100",
    ], "rasm.doc.bounds()")
    assert b["min"] == [-5.0, -5.0]
    assert b["max"] == [100.0, 100.0]


def test_doc_bounds_negative_and_arc(cadcli):
    _, (b,) = jrun(cadcli, [
        "line -10,-3 4,7", "arc 0,0 6 90 270",
    ], "rasm.doc.bounds()")
    # the arc 90..270 sweeps down to y=-6 (tight bbox)
    assert b["min"] == [-10.0, -6.0]
    assert b["max"] == [4.0, 7.0]


def test_doc_bounds_excludes_hatches(cadcli):
    # bounds are identical with and without a hatch (hatch excluded from
    # the kernel's DocBounds computation)
    base = ["circle 0,0 1", "line 0,0 1,1"]
    _, (b2,) = jrun(cadcli, base, "rasm.doc.bounds()")
    _, (b3,) = jrun(cadcli, base + ["py rasm.add_hatch([0], 'SOLID')"],
                    "rasm.doc.bounds()")
    assert b2 == b3 == {"min": [-1.0, -1.0], "max": [1.0, 1.0]}


def test_doc_layers_and_active_layer(cadcli):
    _, (layers, active) = jrun(cadcli, [],
                               "rasm.doc.layers()", "rasm.doc.active_layer()")
    assert len(layers) == 1
    assert active == 0
    _, (layers2, active2) = jrun(cadcli, ["py rasm.add_layer('X')"],
                                 "rasm.doc.layers()", "rasm.doc.active_layer()")
    assert len(layers2) == 2
    assert active2 == 1


def test_doc_layouts(cadcli):
    assert cadcli.py_json("rasm.doc.layouts()") == []


def test_doc_linetypes(cadcli):
    catalog = cadcli.py_json("rasm.doc.linetypes()")
    assert "Continuous" in catalog
    assert len(catalog) == len(set(catalog))


# -- selection / sysvars / view -------------------------------------------------------

def test_selection_empty_headless(cadcli):
    _, (sel,) = jrun(cadcli, ["line 0,0 1,1"], "rasm.selection()")
    assert sel == []


def test_set_selection_headless_noop(cadcli):
    # headless answers an empty validated list (nothing is selected)
    _, (sel,) = jrun(cadcli, ["line 0,0 1,1",
                              "py rasm.set_selection([0])"],
                     "rasm.set_selection([0])")
    assert sel == []


def test_sysvar_returns_none(cadcli):
    res = cadcli.run(["py rasm.sysvar('LTSCALE')"]).expect_ok()
    assert all(not r.startswith("= ") for r in res.replies)


def test_setvar_headless_error(cadcli):
    res = cadcli.run(["py rasm.setvar('LTSCALE', '2')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: sysvars are not available in the headless CLI")


def test_view_defaults(cadcli):
    d = cadcli.py_json("rasm.view()")
    assert d == {"center": [0.0, 0.0], "scale": 1.0}


def test_set_view_and_zoom_extents_ok(cadcli):
    res = cadcli.run([
        "py rasm.set_view((10, 20), 5.0)",
        "py rasm.zoom_extents()",
    ]).expect_ok()
    assert not res.tracebacks


# -- layouts / undo / current style -----------------------------------------------------

def test_set_layout_valid_and_missing(cadcli):
    # headless doc has no layouts, so every name is missing
    res = cadcli.run(["py rasm.set_layout('Model')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no layout named 'Model'")


def test_undo_group_ok(cadcli):
    res = cadcli.run(["py rasm.undo_group()"]).expect_ok()
    assert not res.tracebacks


@pytest.mark.parametrize("color,expect", [
    ("5", "aci 5"), ("0", "aci 0"), ("255", "aci 255"),
    ("None", "aci 7"), ("'bylayer'", "aci 7"),
    ("'byblock'", "byblock"),
])
def test_set_current_color_applies_to_new_entities(cadcli, color, expect):
    # docs §4.8: set_current_color sets the style for the script's NEW
    # entities (ByLayer resolves through the layer)
    _, (d,) = jrun(cadcli, [
        f"py rasm.set_current_color({color})",
        "py rasm.add_line((0,0),(1,1))",
    ], "rasm.doc.get(0)")
    assert d["color"] == expect


def test_set_current_color_invalid(cadcli):
    for bad, needle in [("300", "ValueError: ACI color must be 0..=255"),
                        ("-1", "ValueError: ACI color must be 0..=255")]:
        res = cadcli.run([f"py rasm.set_current_color({bad})"]).expect_ok(
            allow_tracebacks=True)
        res.expect_traceback_containing(needle)


def test_set_current_linetype_applies_to_new_entities(cadcli):
    catalog = cadcli.py_json("rasm.doc.linetypes()")
    other = [n for n in catalog if n != "Continuous"]
    if not other:
        pytest.skip("only Continuous in catalog")
    _, (d,) = jrun(cadcli, [
        f"py rasm.set_current_linetype({other[0]!r})",
        "py rasm.add_line((0,0),(1,1))",
    ], "rasm.doc.get(0)")
    assert d["linetype"] == other[0]


def test_set_current_linetype_missing(cadcli):
    res = cadcli.run(["py rasm.set_current_linetype('Nope')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no linetype named 'Nope'")


def test_set_current_lineweight_grid(cadcli):
    for mm in (0.0, 0.1, 1.0, 0.35):
        res = cadcli.run([f"py rasm.set_current_lineweight({mm})"]).expect_ok()
        assert not res.tracebacks, res.tracebacks
    res = cadcli.run(["py rasm.set_current_lineweight(-1)"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: lineweight mm must be >= 0")


def test_current_style_applies_to_new_entities(cadcli):
    _, (d,) = jrun(cadcli, [
        "py rasm.set_current_color(3)",
        "py rasm.set_current_lineweight(1.5)",
        "py rasm.add_line((0,0),(1,1))",
    ], "rasm.doc.get(0)")
    assert d["color"] == "aci 3"
    assert d["lineweight"] == 1.5


# -- rasm.command ----------------------------------------------------------------------

def test_rasm_command_line_adds_dobject(cadcli):
    res = cadcli.run(["py rasm.command('line 1,1 2,2')"]).expect_ok()
    assert any(r.startswith("= ['+ #0 line (1.0000,1.0000)")
               for r in res.replies)
    _, (count,) = jrun(cadcli, ["py rasm.command('line 1,1 2,2')"],
                       "rasm.doc.count()")
    assert count == 1


def test_rasm_command_returns_transcript_lines(cadcli):
    res = cadcli.run(["py rasm.command('circle 0,0 5')"]).expect_ok()
    vals = [r[2:] for r in res.replies if r.startswith("= ")]
    assert vals and "circle c=(0.0000,0.0000) r=5.0000" in vals[0]


def test_rasm_command_bad_line(cadcli):
    res = cadcli.run(["py rasm.command('bogus x')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: parse error: unknown command 'bogus'")


def test_rasm_command_nested_python_rejected(cadcli):
    res = cadcli.run(["py rasm.command('py 1+1')"]).expect_ok()
    assert any("(nested python not supported headless)" in r
               for r in res.replies)


def test_rasm_command_delete_through_parser(cadcli):
    _, (count, d) = jrun(cadcli, [
        "py rasm.command('line 0,0 10,0')",
        "py rasm.command('circle 0,0 2')",
        "py rasm.command('del 0')",
    ], "rasm.doc.count()", "rasm.doc.get(0)")
    assert count == 1
    assert d["type"] == "circle"
