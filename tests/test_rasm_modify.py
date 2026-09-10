"""rasm move/copy/rotate/scale/mirror + style setters, with Python-side
transform verification. Every scenario runs its ops and read-backs in ONE
process (jrun)."""

import math

import pytest

from tests.cadcli import check_obj, jrun


def rot(p, pivot, deg):
    a = math.radians(deg)
    dx, dy = p[0] - pivot[0], p[1] - pivot[1]
    return (pivot[0] + dx * math.cos(a) - dy * math.sin(a),
            pivot[1] + dx * math.sin(a) + dy * math.cos(a))


def scale(p, pivot, f):
    return (pivot[0] + f * (p[0] - pivot[0]),
            pivot[1] + f * (p[1] - pivot[1]))


def mirror(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    l2 = dx * dx + dy * dy
    t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2
    proj = (a[0] + t * dx, a[1] + t * dy)
    return (2 * proj[0] - p[0], 2 * proj[1] - p[1])


def pts_after(cadcli, ops, index=0):
    """Run `ops` (py lines) + the add, then read entity `index` back."""
    _, (d,) = jrun(cadcli, ops, f"rasm.doc.get({index})")
    return tuple(d["start"]), tuple(d["end"])


def add_line(cadcli, a, b):
    return [f"py rasm.add_line({a}, {b})"]


# -- move ------------------------------------------------------------------------

@pytest.mark.parametrize("dx,dy", [
    (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (-1.0, -1.0), (1e-3, -1e-3),
    (1e6, -1e6), (1.5, 2.25), (-0.5, 0.125),
])
def test_move_grid(cadcli, dx, dy):
    res, (d,) = jrun(cadcli,
                     add_line(cadcli, (0, 0), (10, 0))
                     + [f"py rasm.move([0], {dx}, {dy})"],
                     "rasm.doc.get(0)")
    assert any(r == "= 1" for r in res.replies)
    assert d["start"] == [0 + dx, 0 + dy]
    assert d["end"] == [10 + dx, 0 + dy]


def test_move_multiple_and_mixed_indices(cadcli):
    res, (d0, d1, d2) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.add_line((2,2),(3,3))",
        "py rasm.add_line((4,4),(5,5))",
        "py rasm.move([0, 2], 10, 10)",
    ], "rasm.doc.get(0)", "rasm.doc.get(1)", "rasm.doc.get(2)")
    assert any(r == "= 2" for r in res.replies)
    assert d0["start"] == [10.0, 10.0]
    assert d0["end"] == [11.0, 11.0]
    assert d1["start"] == [2.0, 2.0]
    assert d2["start"] == [14.0, 14.0]


def test_move_all_invalid_indices(cadcli):
    res = cadcli.run(["py rasm.move([9, 99], 1, 1)"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: none of the given entity indices exist")


# -- copy ------------------------------------------------------------------------

def test_copy_returns_new_indices_original_untouched(cadcli):
    res, (d0, d1) = jrun(cadcli, [
        "py rasm.add_line((0,0),(10,0))",
        "py rasm.copy([0], 5, 5)",
    ], "rasm.doc.get(0)", "rasm.doc.get(1)")
    assert any(r == "= [1]" for r in res.replies)
    assert d0["start"] == [0.0, 0.0]
    assert d1["start"] == [5.0, 5.0]
    assert d1["end"] == [15.0, 5.0]


def test_copy_multiple(cadcli):
    res, (d2, d3) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,0))",
        "py rasm.add_line((2,0),(3,0))",
        "py rasm.copy([0, 1], 0, 10)",
    ], "rasm.doc.get(2)", "rasm.doc.get(3)")
    assert any(r == "= [2, 3]" for r in res.replies)
    assert d2["start"] == [0.0, 10.0]
    assert d3["start"] == [2.0, 10.0]


def test_copy_negative_delta(cadcli):
    _, (d1,) = jrun(cadcli, [
        "py rasm.add_line((5,5),(6,5))",
        "py rasm.copy([0], -2.5, -1.5)",
    ], "rasm.doc.get(1)")
    assert d1["start"] == [2.5, 3.5]
    assert d1["end"] == [3.5, 3.5]


def test_copy_all_invalid(cadcli):
    # docs §4.2: "Indices are validated loudly: if none of them exist the
    # call raises."
    res = cadcli.run(["py rasm.copy([5], 1, 1)"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: none of the given entity indices exist")


# -- rotate ----------------------------------------------------------------------

@pytest.mark.parametrize("angle", [0, 45, 90, 180, -90, 270, 360, 123.456])
def test_rotate_angles(cadcli, angle):
    a, b = pts_after(cadcli,
                     add_line(cadcli, (1, 2), (6, 2))
                     + [f"py rasm.rotate([0], (1, 2), {angle})"])
    assert a == pytest.approx((1, 2), abs=1e-9)
    assert b == pytest.approx(rot((6, 2), (1, 2), angle), abs=1e-9)


@pytest.mark.parametrize("pivot", [(0, 0), (3, 3), (100, -50)])
def test_rotate_pivots(cadcli, pivot):
    a, b = pts_after(cadcli,
                     add_line(cadcli, (10, 0), (20, 0))
                     + [f"py rasm.rotate([0], {pivot}, 90)"])
    assert a == pytest.approx(rot((10, 0), pivot, 90), abs=1e-9)
    assert b == pytest.approx(rot((20, 0), pivot, 90), abs=1e-9)


def test_rotate_multiple_entities(cadcli):
    res, (d0, d1) = jrun(cadcli, [
        "py rasm.add_line((0,0),(10,0))",
        "py rasm.add_line((0,10),(10,10))",
        "py rasm.rotate([0, 1], (0, 0), 180)",
    ], "rasm.doc.get(0)", "rasm.doc.get(1)")
    assert any(r == "= 2" for r in res.replies)
    assert d0["end"] == pytest.approx([-10.0, 0.0], abs=1e-9)
    assert d1["start"] == pytest.approx([0.0, -10.0], abs=1e-9)


# -- scale -----------------------------------------------------------------------

@pytest.mark.parametrize("factor", [1.0, 0.5, 2.0, 1e-6, 1e6, 1.25])
def test_scale_factors(cadcli, factor):
    a, b = pts_after(cadcli,
                     add_line(cadcli, (4, 4), (8, 4))
                     + [f"py rasm.scale([0], (4, 4), {factor})"])
    assert a == pytest.approx(scale((4, 4), (4, 4), factor), abs=1e-9)
    assert b == pytest.approx(scale((8, 4), (4, 4), factor), abs=1e-9)


@pytest.mark.parametrize("factor", ["0", "-2", "-0.5"])
def test_scale_invalid_factor_rejected(cadcli, factor):
    # docs §4.2: "factor must be > 0"
    res = cadcli.run([
        "py rasm.add_line((10,0),(20,0))",
        f"py rasm.scale([0], (0, 0), {factor})",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("ValueError: scale factor must be > 0")


# -- mirror ----------------------------------------------------------------------

@pytest.mark.parametrize("axis", [
    ((0, 0), (1, 0)),   # horizontal
    ((0, 0), (0, 1)),   # vertical
    ((0, 0), (1, 1)),   # 45°
    ((5, 5), (5, 15)),  # offset vertical
])
def test_mirror_axes(cadcli, axis):
    a, b = pts_after(cadcli,
                     add_line(cadcli, (1, 1), (4, 2))
                     + [f"py rasm.mirror([0], {axis[0]}, {axis[1]})"])
    assert a == pytest.approx(mirror((1, 1), *axis), abs=1e-9)
    assert b == pytest.approx(mirror((4, 2), *axis), abs=1e-9)


# -- chain ------------------------------------------------------------------------

def test_transform_chain_math(cadcli):
    _, (d0,) = jrun(cadcli, [
        "py rasm.add_line((2,1),(8,1))",
        "py rasm.move([0], 3, 4)",
        "py rasm.rotate([0], (5, 5), 90)",
        "py rasm.scale([0], (5, 5), 2)",
        "py rasm.mirror([0], (0, 0), (1, 0))",
    ], "rasm.doc.get(0)")
    assert d0["start"] == pytest.approx([5, -5], abs=1e-6)
    assert d0["end"] == pytest.approx([5, -17], abs=1e-6)


# -- style setters -----------------------------------------------------------------

@pytest.mark.parametrize("color,expect", [
    (0, "aci 0"), (1, "aci 1"), (7, "aci 7"), (255, "aci 255"),
    # ByLayer resolves through the layer (default Aci(7)); observed behavior.
    ("'bylayer'", "aci 7"), ("None", "aci 7"),
    ("'byblock'", "byblock"),
])
def test_set_color_grid(cadcli, color, expect):
    _, (d0,) = jrun(cadcli,
                    add_line(cadcli, (0, 0), (1, 1))
                    + [f"py rasm.set_color([0], {color})"],
                    "rasm.doc.get(0)")
    assert d0["color"] == expect


@pytest.mark.parametrize("color,needle", [
    ("256", "ValueError: ACI color must be 0..=255"),
    ("-1", "ValueError: ACI color must be 0..=255"),  # ints must be 0..=255
    ("-2", "ValueError: ACI color must be 0..=255"),
    ("-3", "ValueError: ACI color must be 0..=255"),
    ("'bogus'", "ValueError: unknown color 'bogus'"),
])
def test_set_color_errors(cadcli, color, needle):
    res = cadcli.run([f"py rasm.set_color([0], {color})"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(needle)


def test_set_layer_of(cadcli):
    _, (d0,) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.add_layer('Walls', set_current=False)",
        "py rasm.set_layer_of([0], 'Walls')",
    ], "rasm.doc.get(0)")
    assert d0["layer"] == "Walls"


def test_set_layer_of_missing_layer(cadcli):
    res = cadcli.run(["py rasm.set_layer_of([0], 'Nope')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no layer named 'Nope'")


def test_set_linetype_grid(cadcli):
    for code, expect in [("''", "Continuous"), ("'bylayer'", "Continuous"),
                         ("None", "Continuous")]:
        _, (d0,) = jrun(cadcli,
                        add_line(cadcli, (0, 0), (1, 1))
                        + [f"py rasm.set_linetype([0], {code})"],
                        "rasm.doc.get(0)")
        assert d0["linetype"] == expect


def test_set_linetype_from_catalog(cadcli):
    catalog = cadcli.py_json("rasm.doc.linetypes()")
    other = [n for n in catalog if n != "Continuous"]
    if not other:
        pytest.skip("linetype catalog has only Continuous")
    _, (d0,) = jrun(cadcli,
                    add_line(cadcli, (0, 0), (1, 1))
                    + [f"py rasm.set_linetype([0], {other[0]!r})"],
                    "rasm.doc.get(0)")
    assert d0["linetype"] == other[0]


def test_set_linetype_missing(cadcli):
    res = cadcli.run(["py rasm.set_linetype([0], 'BogusLT')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: no linetype named 'BogusLT'")


@pytest.mark.parametrize("mm", [0.0, 0.1, 1.0, 0.35, 2.5])
def test_set_lineweight_grid(cadcli, mm):
    _, (d0,) = jrun(cadcli,
                    add_line(cadcli, (0, 0), (1, 1))
                    + [f"py rasm.set_lineweight([0], {mm})"],
                    "rasm.doc.get(0)")
    assert d0["lineweight"] == pytest.approx(mm, abs=1e-6)


def test_set_lineweight_negative_is_bylayer(cadcli):
    _, (d0, d1) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.add_line((5,5),(6,6))",
        "py rasm.set_lineweight([0], -1)",
    ], "rasm.doc.get(0)", "rasm.doc.get(1)")
    assert d0["lineweight"] == d1["lineweight"]


def test_set_visible_roundtrip(cadcli):
    _, (d0,) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.set_visible([0], False)",
    ], "rasm.doc.get(0)")
    assert d0["visible"] is False
    _, (d0,) = jrun(cadcli, [
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.set_visible([0], False)",
        "py rasm.set_visible([0], True)",
    ], "rasm.doc.get(0)")
    assert d0["visible"] is True
    # hidden entities still exist in the scene dump
    res = cadcli.run([
        "py rasm.add_line((0,0),(1,1))",
        "py rasm.set_visible([0], False)",
        "list",
    ]).expect_ok()
    assert len(res.dobjects) == 1
