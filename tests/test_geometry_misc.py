"""ellipse / polyline / point parameter grids."""

import math

import pytest

from tests import geommath
from tests.cadcli import check_obj, jrun

DEG = 180.0 / math.pi


# -- ellipse ----------------------------------------------------------------------

@pytest.mark.parametrize("center,major_end,minor,exp_rot", [
    ((0.0, 0.0), (5.0, 0.0), 2.0, 0.0),
    ((0.0, 0.0), (0.0, 5.0), 2.0, 90.0),
    ((0.0, 0.0), (-5.0, 0.0), 2.0, 180.0),
    ((0.0, 0.0), (0.0, -5.0), 2.0, -90.0),
    ((0.0, 0.0), (5.0, 5.0), 2.0, 45.0),
    ((0.0, 0.0), (-5.0, 5.0), 2.0, 135.0),
    ((2.0, -3.0), (7.0, -3.0), 1.5, 0.0),
    ((0.0, 0.0), (1000000.0, 0.0), 500000.0, 0.0),
    ((0.0, 0.0), (3.0, 4.0), 1.0, math.degrees(math.atan2(4, 3))),
])
def test_ellipse_valid(cadcli, center, major_end, minor, exp_rot):
    cx, cy = center
    mx, my = major_end
    exp = geommath.ellipse_center_major_minor(center, major_end, minor)
    assert exp is not None
    a = math.hypot(mx - cx, my - cy)
    ratio = min(minor / a, 1.0)
    res, (d,) = jrun(cadcli, [f"ellipse {cx},{cy} {mx},{my} {minor}"],
                     "rasm.doc.get(0)")
    assert len(res.dobjects) == 1
    check_obj(res.dobjects[0], "ellipse", center=center, semi_major=a, ratio=ratio,
              rot_deg=exp_rot)
    assert d["type"] == "ellipse"
    assert d["center"] == list(center)
    assert d["major"] == [mx - cx, my - cy]
    assert abs(d["ratio"] - ratio) < 1e-9


def test_ellipse_minor_longer_than_major_clamps_ratio(cadcli):
    res = cadcli.run(["ellipse 0,0 5,0 10"]).expect_ok()
    check_obj(res.dobjects[0], "ellipse", semi_major=5.0, ratio=1.0, rot_deg=0.0)


@pytest.mark.parametrize("cmd", [
    "ellipse 5,5 5,5 2",      # major_end == center → zero major
    "ellipse 0,0 5,0 0",      # zero minor
    "ellipse 0,0 5,0 -1",     # negative minor
    "ellipse 0,0 0,0 0",
])
def test_ellipse_degenerate_errors(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any("degenerate inputs (zero major or minor)" in e
               for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []


# -- polyline ----------------------------------------------------------------------

def test_polyline_open_2_verts(cadcli):
    res = cadcli.run(["polyline 0,0 10,0"]).expect_ok()
    check_obj(res.dobjects[0], "polyline", verts=2, closed=False,
              length=10.0)


def test_polyline_closed_suffixes(cadcli):
    for suffix in ("close", "closed"):
        res = cadcli.run(
            [f"polyline 0,0 10,0 10,10 0,10 {suffix}"]).expect_ok()
        check_obj(res.dobjects[0], "polyline", verts=4, closed=True,
                  length=40.0)


def test_polyline_3_verts_length(cadcli):
    res = cadcli.run(["polyline 0,0 10,0 10,10"]).expect_ok()
    check_obj(res.dobjects[0], "polyline", verts=3, closed=False,
              length=20.0)


def test_polyline_duplicate_points_allowed(cadcli):
    res = cadcli.run(["polyline 0,0 0,0 5,5 5,5"]).expect_ok()
    check_obj(res.dobjects[0], "polyline", verts=4, closed=False,
              length=math.sqrt(50.0))


def test_polyline_large_count_and_length(cadcli):
    pts = " ".join(f"{i},0" for i in range(1000))
    res, (d,) = jrun(cadcli, [f"polyline {pts}"], "rasm.doc.get(0)")
    check_obj(res.dobjects[0], "polyline", verts=1000, closed=False,
              length=999.0)
    assert len(d["points"]) == 1000
    assert d["closed"] is False


def test_polyline_json_roundtrip(cadcli):
    _, (d,) = jrun(cadcli, ["polyline 0,0 1,1 2,0 close"],
                   "rasm.doc.get(0)")
    assert d["points"] == [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]
    assert d["closed"] is True
    assert d["bulges"] == [0.0, 0.0, 0.0]


@pytest.mark.parametrize("cmd,needle", [
    ("polyline 0,0", "usage: polyline"),
    ("polyline 0,0 1,1 2,2 close extra", "expected x,y, got 'close'"),
    ("polyline 0,0 a,b", "bad x: 'a'"),
    ("polyline 0,0 1,1 x", "expected x,y, got 'x'"),
])
def test_polyline_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []


# -- point -------------------------------------------------------------------------

@pytest.mark.parametrize("at", [
    (0.0, 0.0), (3.0, 4.0), (-2.5, 7.25), (1e6, -1e6), (-0.0, 0.0),
])
def test_point_valid(cadcli, at):
    res, (d,) = jrun(cadcli, [f"point {at[0]},{at[1]}"], "rasm.doc.get(0)")
    check_obj(res.dobjects[0], "point", at=at, style=0, size=0.0)
    assert d["type"] == "point"
    assert d["at"] == list(at)
    assert d["pdmode"] == 0
    assert d["pdsize"] == 0.0


@pytest.mark.parametrize("cmd,needle", [
    ("point a,b", "bad x: 'a'"),
    ("point 1,2 3,4", "usage: point"),
    ("point 1", "expected x,y, got '1'"),
])
def test_point_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []
