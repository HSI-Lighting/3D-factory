"""rasm hatch API: pattern catalog, add_hatch boundary/pattern grids,
hatch_at headless error, hatch + transform behavior."""

import pytest

from tests.cadcli import check_obj, jrun

HATCH_PATTERNS = ["SOLID", "ANSI31", "ANSI32", "ANSI33", "ANSI37", "CROSS",
                  "NET", "ANGLE", "BRICK", "TILE", "CONCRETE", "EARTH",
                  "LINE", "DOTS", "DOUBLE", "DASH", "SQGRID", "CONCENTRIC"]


def test_hatch_patterns_catalog(cadcli):
    pats = cadcli.py_json("rasm.hatch_patterns()")
    assert "SOLID" in pats
    assert len(pats) == len(set(pats))
    assert set(pats) == set(HATCH_PATTERNS)


@pytest.mark.parametrize("pattern,expect", [
    ("SOLID", "Solid"),
    ("ANSI31", 'Pattern { name: "ANSI31"'),
    ("ansi31", 'Pattern { name: "ANSI31"'),   # case-insensitive canonicalized
    ("brick", 'Pattern { name: "BRICK"'),
])
def test_add_hatch_patterns(cadcli, pattern, expect):
    res, (j,) = jrun(cadcli, [
        "py rasm.add_circle((0,0), 5)",
        f"py rasm.add_hatch([0], {pattern!r})",
    ], "rasm.doc.get(1)")
    assert any(r == "= 1" for r in res.replies)
    assert len(res.dobjects) == 2
    d = res.dobjects[1]
    check_obj(d, "hatch", boundary_loops=1)
    assert expect in d["pattern"]
    assert j["type"] == "hatch"
    assert j["boundary_loops"] == 1


def test_add_hatch_boundary_kinds(cadcli):
    res, (j,) = jrun(cadcli, [
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_polyline([(0,0),(10,0),(10,10),(0,10)], closed=True)",
        "py rasm.add_ellipse((0,0), (5,0), 0.5)",
        "py rasm.add_hatch([0, 1, 2], 'ANSI31')",
    ], "rasm.doc.get(3)")
    assert any(r == "= 3" for r in res.replies)
    check_obj(res.dobjects[3], "hatch", boundary_loops=3)
    assert j["boundary_loops"] == 3


def test_add_hatch_open_boundary_rejected(cadcli):
    res = cadcli.run([
        "py rasm.add_line((0,0),(10,0))",
        "py rasm.add_hatch([0], 'SOLID')",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: add_hatch: none of the given indices is a closed "
        "boundary")
    assert len(res.dobjects) == 1


def test_add_hatch_open_polyline_rejected(cadcli):
    res = cadcli.run([
        "py rasm.add_polyline([(0,0),(10,0),(10,10)])",
        "py rasm.add_hatch([0], 'SOLID')",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: add_hatch: none of the given indices is a closed "
        "boundary")


def test_add_hatch_mixed_valid_and_invalid(cadcli):
    # observed: only the closed boundary (circle) joins the hatch; the open
    # line and point are silently skipped.
    res, (j,) = jrun(cadcli, [
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_line((0,0),(10,0))",
        "py rasm.add_point((1,1))",
        "py rasm.add_hatch([0, 1, 2], 'SOLID')",
    ], "rasm.doc.get(3)")
    assert any(r == "= 3" for r in res.replies)
    assert j["type"] == "hatch"
    assert j["boundary_loops"] == 1


def test_add_hatch_out_of_range(cadcli):
    res = cadcli.run([
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_hatch([5], 'SOLID')",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: add_hatch: none of the given indices is a closed "
        "boundary")


def test_add_hatch_unknown_pattern_lists_catalog(cadcli):
    res = cadcli.run([
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_hatch([0], 'NOPE')",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: no hatch pattern 'NOPE' — available: "
        "SOLID, ANSI31")
    tb = next(t for t in res.tracebacks if "no hatch pattern" in t)
    assert "CONCENTRIC" in tb


def test_hatch_at_headless_error(cadcli):
    res = cadcli.run([
        "py rasm.add_circle((0,0), 5)",
        "py rasm.hatch_at((0,0), 'SOLID')",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: hatch boundary tracing is not available in the "
        "headless CLI — use add_hatch with explicit boundary indices")


def test_hatch_scene_dump_shape(cadcli):
    res = cadcli.run([
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_hatch([0], 'ANSI31')",
        "list",
    ]).expect_ok()
    assert any("hatch (1 boundary loops, Pattern { name: \"ANSI31\""
               in r for r in res.replies)


def test_hatch_transform_preserves_boundary_handles(cadcli):
    res, (j, c) = jrun(cadcli, [
        "py rasm.add_circle((0,0), 5)",
        "py rasm.add_hatch([0], 'ANSI31')",
        "py rasm.move([0, 1], 10, 0)",
        "py rasm.rotate([0, 1], (0, 0), 90)",
        "py rasm.scale([0, 1], (0, 0), 2)",
    ], "rasm.doc.get(1)", "rasm.doc.get(0)")
    assert any(r == "= 2" for r in res.replies)
    assert j["type"] == "hatch"
    assert j["boundary_loops"] == 1
    # boundary circle moved with the hatch: move(10,0) → rotate 90° → scale 2
    assert c["center"] == pytest.approx([0.0, 20.0], abs=1e-9)
