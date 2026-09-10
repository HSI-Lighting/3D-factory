"""`line` parameter grid: valid values, scene + json read-back, errors."""

import pytest

from tests.cadcli import check_obj, jrun


@pytest.mark.parametrize("a,b", [
    ((0.0, 0.0), (10.0, 0.0)),
    ((-5.0, -5.0), (5.0, 5.0)),
    ((1.5, 2.25), (-3.75, 0.125)),
    ((1000000.0, 0.0), (1000000.0, 1000000.0)),
    ((1000.0, 1000.0), (2000.0, 2000.0)),   # scientific input form
    ((5.0, 5.0), (5.0, 5.0)),               # same-point line
    ((10.0, 0.0), (0.0, 0.0)),              # swapped endpoints
    ((-0.0, 0.0), (1.0, -0.0)),
])
def test_line_valid(cadcli, a, b):
    cmd = f"line {a[0]},{a[1]} {b[0]},{b[1]}"
    res, (d,) = jrun(cadcli, [cmd], "rasm.doc.get(0)")
    assert len(res.dobjects) == 1
    assert any(r == f"+ #0 {res.dobjects[0].raw}" for r in res.replies)
    check_obj(res.dobjects[0], "line", a=a, b=b)
    assert d["type"] == "line"
    assert d["start"] == list(a)
    assert d["end"] == list(b)


def test_line_tiny_coordinates_exact_via_json(cadcli):
    # 1e-6 rounds to 0.0000 in the scene dump — exact value via json.
    res, (d,) = jrun(cadcli, ["line 0.000001,0 0,0.000001"],
                     "rasm.doc.get(0)")
    check_obj(res.dobjects[0], "line", a=(0.0, 0.0), b=(0.0, 0.0))
    assert d["start"] == [0.000001, 0.0]
    assert d["end"] == [0.0, 0.000001]


def test_line_negative_zero_and_frac_json(cadcli):
    _, (d,) = jrun(cadcli, ["line -1.5,2.25 3.75,-0.125"],
                   "rasm.doc.get(0)")
    assert d["start"] == [-1.5, 2.25]
    assert d["end"] == [3.75, -0.125]


def test_line_two_lines_indices_and_dump_order(cadcli):
    res = cadcli.run(["line 0,0 1,1", "line 2,2 3,3"]).expect_ok()
    assert len(res.dobjects) == 2
    check_obj(res.dobjects[0], "line", a=(0, 0), b=(1, 1))
    check_obj(res.dobjects[1], "line", a=(2, 2), b=(3, 3))
    assert any(r.startswith("+ #1 ") for r in res.replies)


@pytest.mark.parametrize("cmd,needle", [
    ("line 0,0", "usage: line"),
    ("line 0,0 1,1 2,2", "usage: line"),
    ("line a,b c,d", "bad x: 'a'"),
    ("line 0,0 x", "expected x,y, got 'x'"),
    ("line 1 2,3", "expected x,y, got '1'"),
    ("line 0,0 1,2 3", "usage: line"),
])
def test_line_errors(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)
    assert res.dobjects == []
