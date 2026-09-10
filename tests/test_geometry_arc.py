"""All five arc constructors, full permutation + edge grid.

Expected values are computed by independent Python ports of the kernel's
construction math (tests/geommath.py), then compared against the scene dump
(±1e-4, the describe() rounding) and the exact json read-back.
"""

import math

import pytest

from tests import geommath
from tests.cadcli import check_obj, jrun

DEG = 180.0 / math.pi


def assert_arc_scene(cadcli, cmd, exp, tol=1e-4):
    """exp = (cx, cy, r, start_deg, sweep_deg) or None (must fail)."""
    res = cadcli.run([cmd]).expect_ok()
    if exp is None:
        assert res.dobjects == [], f"expected failure but built {res.dobjects}"
        return res
    assert len(res.dobjects) == 1, res.raw_stdout
    check_obj(res.dobjects[0], "arc", center=(exp[0], exp[1]),
              radius=exp[2], start_deg=exp[3], sweep_deg=exp[4], tol=tol)
    _, (d,) = jrun(cadcli, [cmd], "rasm.doc.get(0)")
    assert d["type"] == "arc"
    assert abs(d["center"][0] - exp[0]) < 1e-9
    assert abs(d["center"][1] - exp[1]) < 1e-9
    assert abs(d["radius"] - exp[2]) < 1e-9
    assert abs(d["start_deg"] - exp[3]) < 1e-6
    assert abs(d["sweep_deg"] - exp[4]) < 1e-6
    return res


# -- arc cx,cy r start end ------------------------------------------------------

@pytest.mark.parametrize("args", [
    (0, 0, 5, 0, 90),        # start < end
    (0, 0, 5, 350, 10),      # wrap-around: sweep 20
    (0, 0, 5, 45, 45),       # start == end → full circle
    (0, 0, 5, -90, 90),      # negative angles
    (0, 0, 5, 720, 810),     # angles > 360
    (0, 0, 5, -450, 90),     # negative wrap
    (2, 3, 5, 30, 120),      # non-origin center
    (0, 0, 1e6, -30, 60),    # huge radius
    (0, 0, 0.5, 10, 370),    # sweep 360 via >360 end
])
def test_arc_center_radius_deg(cadcli, args):
    cx, cy, r, sd, ed = args
    exp = geommath.arc_center_radius_deg(cx, cy, r, sd, ed)
    assert_arc_scene(cadcli, f"arc {cx},{cy} {r} {sd} {ed}", exp)


def test_arc_tiny_radius_exact_json(cadcli):
    res, (d,) = jrun(cadcli, ["arc 0,0 0.000001 0 90"], "rasm.doc.get(0)")
    check_obj(res.dobjects[0], "arc", radius=0.0, start_deg=0, sweep_deg=90)
    assert d["radius"] == 0.000001


@pytest.mark.parametrize("cmd,needle", [
    ("arc 0,0 0 0 90", "radius must be > 0"),
    ("arc 0,0 -5 0 90", "radius must be > 0"),
    ("arc 0,0 x 0 90", "bad radius"),
    ("arc 0,0 5 x 90", "bad start angle"),
    ("arc 0,0 5 0 x", "bad end angle"),
    ("arc 0,0 5 0", "usage: arc"),
])
def test_arc_center_radius_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []


# -- arc3p -----------------------------------------------------------------------

ARC3P_PTS = [(0.0, 0.0), (5.0, 0.0), (0.0, 5.0)]


@pytest.mark.parametrize("perm", [
    (0, 1, 2), (0, 2, 1), (1, 0, 2), (1, 2, 0), (2, 0, 1), (2, 1, 0),
])
def test_arc3p_all_point_order_permutations(cadcli, perm):
    pts = [ARC3P_PTS[i] for i in perm]
    exp = geommath.arc_three_points(*pts)
    assert exp is not None
    cmd = "arc3p " + " ".join(f"{p[0]},{p[1]}" for p in pts)
    assert_arc_scene(cadcli, cmd, exp)


def test_arc3p_other_geometry(cadcli):
    # right triangle, hypotenuse = diameter: centre at midpoint of p2p3.
    exp = geommath.arc_three_points((1.0, 2.0), (4.0, 2.0), (1.0, 6.0))
    assert_arc_scene(cadcli, "arc3p 1,2 4,2 1,6", exp)


def test_arc3p_large_coordinates(cadcli):
    exp = geommath.arc_three_points((1000, 1000), (1050, 1000), (1000, 1050))
    assert_arc_scene(cadcli, "arc3p 1000,1000 1050,1000 1000,1050", exp)


def test_arc3p_small_legs_scale_free(cadcli):
    # A genuine right angle with legs 1e-4 must still build (G8 regression guard).
    exp = geommath.arc_three_points((0, 0), (1e-4, 0), (0, 1e-4))
    assert exp is not None, "small right-angle triple must build an arc"
    assert_arc_scene(cadcli, "arc3p 0,0 0.0001,0 0,0.0001", exp,
                     tol=1e-3)  # scene rounds to 4 decimals


@pytest.mark.parametrize("cmd", [
    "arc3p 0,0 5,0 10,0",        # collinear
    "arc3p 0,0 5,0 5,0",         # duplicate points
    "arc3p 0,0 0,0 0,0",         # all identical
    "arc3p 0,0 5,0 10,0.0000000001",  # near-collinear
])
def test_arc3p_collinear_errors(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any("three points are collinear, no arc" in e for e in res.errors), (
        cmd, res.errors)
    assert res.dobjects == []


# -- arcse -----------------------------------------------------------------------

@pytest.mark.parametrize("cmd,exp", [
    ("arcse 0,0 5,0 0,5",
     (0.0, 0.0, 5.0, 0.0, 90.0)),
    ("arcse 0,0 0,5 5,0",           # swapped start/end → CCW long way
     (0.0, 0.0, 5.0, 90.0, 270.0)),
    ("arcse 0,0 5,0 5,0",           # start == end → full circle
     (0.0, 0.0, 5.0, 0.0, 360.0)),
    ("arcse 2,3 7,3 2,8",           # shifted geometry
     (2.0, 3.0, 5.0, 0.0, 90.0)),
    ("arcse 0,0 -5,0 5,0",          # start at 180°
     (0.0, 0.0, 5.0, 180.0, 180.0)),
    ("arcse 0,0 5,0 0,20",          # end distance ignored (only its angle)
     (0.0, 0.0, 5.0, 0.0, 90.0)),
])
def test_arcse(cadcli, cmd, exp):
    assert_arc_scene(cadcli, cmd, exp)


@pytest.mark.parametrize("cmd", [
    "arcse 0,0 0,0 5,0",        # start == center → zero radius
    "arcse 0,0 0,0 0,0",
])
def test_arcse_errors(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any("zero radius (start coincides with center)" in e
               for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []


def test_arcse_arity_error(cadcli):
    res = cadcli.run(["arcse 0,0 5,0"]).expect_ok(allow_errors=True)
    assert any("usage: arcse cx,cy start end" in e for e in res.errors)
    assert res.dobjects == []


# -- arccr -----------------------------------------------------------------------

@pytest.mark.parametrize("cmd,exp", [
    ("arccr 0,0 10,0 5",            # semicircle, default minor
     (5.0, 0.0, 5.0, 180.0, 180.0)),
    ("arccr 0,0 10,0 5 major",      # equal arcs — same geometry
     (5.0, 0.0, 5.0, 180.0, 180.0)),
    ("arccr 0,0 10,0 6",            # chord < 2r, minor
     None),
    ("arccr 0,0 10,0 6 major",      # chord < 2r, major
     None),
    ("arccr 0,0 10,0 6 minor",      # explicit minor
     None),
    ("arccr -3,0 3,0 5",            # shifted chord
     None),
    ("arccr 0,0 10,0 25",           # nearly straight minor arc
     None),
    ("arccr 0,0 10,0 25 major",
     None),
])
def test_arccr(cadcli, cmd, exp):
    if exp is None:
        exp = _arccr_expected(cmd)
        assert exp is not None, f"{cmd} should build an arc"
    assert_arc_scene(cadcli, cmd, exp)


def _arccr_expected(cmd):
    toks = cmd.split()
    s = (float(toks[1].split(",")[0]), float(toks[1].split(",")[1]))
    e = (float(toks[2].split(",")[0]), float(toks[2].split(",")[1]))
    r = float(toks[3])
    major = len(toks) > 4 and toks[4].lower() == "major"
    return geommath.arc_chord_radius(s, e, r, major)


@pytest.mark.parametrize("cmd,needle", [
    ("arccr 0,0 10,0 4", "chord longer than 2r, or zero inputs"),
    ("arccr 0,0 10,0 0", "chord longer than 2r, or zero inputs"),
    ("arccr 0,0 10,0 -5", "chord longer than 2r, or zero inputs"),
    ("arccr 0,0 0,0 5", "chord longer than 2r, or zero inputs"),
    ("arccr 0,0 10,0 5 weird", "expected 'major' or 'minor', got 'weird'"),
    ("arccr 0,0 10,0 5 6", "expected 'major' or 'minor', got '6'"),
    ("arccr 0,0 10,0", "usage: arccr"),
    ("arccr 0,0 10,0 5 minor extra", "usage: arccr"),
])
def test_arccr_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []


# -- arccl -----------------------------------------------------------------------

@pytest.mark.parametrize("cmd", [
    "arccl 0,0 10,0 20",
    "arccl 0,0 10,0 20 left",
    "arccl 0,0 10,0 20 right",
    "arccl 0,0 10,0 12",
    "arccl 0,0 10,0 12 left",
    "arccl 0,0 10,0 12 right",
    "arccl 0,0 10,0 10.5",
    "arccl 0,0 10,0 40",
    "arccl 5,5 15,5 25",
    "arccl 5,5 5,15 25",
])
def test_arccl(cadcli, cmd):
    toks = cmd.split()
    s = (float(toks[1].split(",")[0]), float(toks[1].split(",")[1]))
    e = (float(toks[2].split(",")[0]), float(toks[2].split(",")[1]))
    length = float(toks[3])
    flip = len(toks) > 4 and toks[4].lower() == "right"
    exp = geommath.arc_chord_length(s, e, length, flip)
    assert exp is not None, f"{cmd} should build an arc"
    assert_arc_scene(cadcli, cmd, exp, tol=2e-3)  # numerical solver


@pytest.mark.parametrize("cmd,needle", [
    ("arccl 0,0 10,0 5", "chord longer than arc length, or degenerate"),
    ("arccl 0,0 10,0 0", "chord longer than arc length, or degenerate"),
    ("arccl 0,0 10,0 10", "chord longer than arc length, or degenerate"),
    ("arccl 0,0 10,0 -3", "chord longer than arc length, or degenerate"),
    ("arccl 0,0 0,0 5", "chord longer than arc length, or degenerate"),
    ("arccl 0,0 10,0 12 weird", "expected 'left' or 'right', got 'weird'"),
    ("arccl 0,0 10,0 12 3", "expected 'left' or 'right', got '3'"),
    ("arccl 0,0 10,0", "usage: arccl"),
])
def test_arccl_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []
