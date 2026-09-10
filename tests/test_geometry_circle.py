"""`circle` parameter grid: centers, radii, errors."""

import pytest

from tests.cadcli import check_obj, jrun


@pytest.mark.parametrize("c,r", [
    ((0.0, 0.0), 1.0),
    ((0.0, 0.0), 0.5),
    ((-5.0, -5.0), 2.5),
    ((1000000.0, 1000000.0), 1000000.0),
    ((0.0, 0.0), 0.1),           # fractional
    ((3.25, -1.75), 2.125),
])
def test_circle_valid(cadcli, c, r):
    res, (d,) = jrun(cadcli, [f"circle {c[0]},{c[1]} {r}"],
                     "rasm.doc.get(0)")
    assert len(res.dobjects) == 1
    check_obj(res.dobjects[0], "circle", center=c, radius=r)
    assert d["type"] == "circle"
    assert d["center"] == list(c)
    assert d["radius"] == r


def test_circle_tiny_radius_scene_rounds_to_zero(cadcli):
    res, (d,) = jrun(cadcli, ["circle 0,0 0.000001"], "rasm.doc.get(0)")
    check_obj(res.dobjects[0], "circle", center=(0, 0), radius=0.0)
    assert d["radius"] == 0.000001


def test_circle_trailing_space_and_int_forms(cadcli):
    res = cadcli.run(["circle 0,0 2 "]).expect_ok()
    assert len(res.dobjects) == 1
    check_obj(res.dobjects[0], "circle", radius=2.0)


def test_circle_scientific_notation(cadcli):
    res = cadcli.run(["circle 1e3,1e3 2.5e2"]).expect_ok()
    check_obj(res.dobjects[0], "circle", center=(1000, 1000), radius=250.0)


@pytest.mark.parametrize("cmd,needle", [
    ("circle 0,0 0", "radius must be > 0"),
    ("circle 0,0 -1", "radius must be > 0"),
    ("circle 0,0 -0.0001", "radius must be > 0"),
    ("circle 0,0 abc", "bad radius"),
    ("circle 0,0", "usage: circle"),
    ("circle 0,0 1 2", "usage: circle"),
    ("circle a,b 1", "bad x: 'a'"),
    ("circle 0,0", "usage: circle"),
])
def test_circle_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []
