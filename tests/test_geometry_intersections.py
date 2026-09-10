"""Scene intersection math: per-pair expected points computed from an
independent Python model (tests/geommath.py), compared against the
`=== intersections ===` dump (tolerance 1e-4) including the
`[dobjects #i ∩ #j]` index pairs."""

import math

import pytest

from tests import geommath
from tests.cadcli import TOL, check_obj


def _roots(f, lo, hi, n=8192):
    """All roots of f on [lo, hi] via sign-change scan + bisection."""
    out = []
    prev_x, prev_y = lo, f(lo)
    step = (hi - lo) / n
    for k in range(1, n + 1):
        x = lo + k * step
        y = f(x)
        if prev_y == 0.0:
            out.append(prev_x)
        elif prev_y * y < 0:
            a, b = prev_x, x
            for _ in range(60):
                m = (a + b) / 2
                if f(a) * f(m) <= 0:
                    b = m
                else:
                    a = m
            out.append((a + b) / 2)
        prev_x, prev_y = x, y
    return out


def circle_ellipse(c_center, r, e_center, major, ratio):
    """Circle ∩ ellipse (numeric, axis-agnostic)."""
    ux, uy = major
    a = math.hypot(ux, uy)
    if a < 1e-12:
        return []
    vx, vy = -uy / a * a * ratio, ux / a * a * ratio
    cx, cy = c_center
    ex, ey = e_center

    def f(t):
        x = ex + ux * math.cos(t) + vx * math.sin(t)
        y = ey + uy * math.cos(t) + vy * math.sin(t)
        return (x - cx) ** 2 + (y - cy) ** 2 - r * r

    pts = []
    for t in _roots(f, 0.0, 2 * math.pi):
        x = ex + ux * math.cos(t) + vx * math.sin(t)
        y = ey + uy * math.cos(t) + vy * math.sin(t)
        if not any(math.hypot(x - px, y - py) < 1e-6 for px, py in pts):
            pts.append((x, y))
    return pts


def _geom_model(dobj):
    k = dobj.kind
    if k == "line":
        return ("line", dobj["a"], dobj["b"])
    if k == "circle":
        return ("circle", dobj["center"], dobj["radius"])
    if k == "arc":
        return ("arc", dobj["center"], dobj["radius"],
                dobj["start_deg"], dobj["sweep_deg"])
    if k == "ellipse":
        a = dobj["semi_major"]
        rot = math.radians(dobj["rot_deg"])
        major = (a * math.cos(rot), a * math.sin(rot))
        return ("ellipse", dobj["center"], major, dobj["ratio"])
    return (k,)


def _intersect_pair(m1, m2):
    def as_circle(m):
        if m[0] == "circle":
            return (m[1], m[2])
        if m[0] == "arc":
            return (m[1], m[2])
        return None

    def as_arc(m):
        if m[0] == "arc":
            return (m[1], m[2], m[3], m[4])
        return None

    k1, k2 = m1[0], m2[0]
    if k1 == "line" and k2 == "line":
        p = geommath.line_line(m1[1], m1[2], m2[1], m2[2])
        return [p] if p else []
    if k1 == "line":
        if k2 == "circle":
            return geommath.line_circle(m1[1], m1[2], m2[1], m2[2])
        if k2 == "arc":
            return geommath.line_arc(m1[1], m1[2], m2[1], m2[2],
                                     m2[3], m2[4])
        if k2 == "ellipse":
            return geommath.line_ellipse(m1[1], m1[2], m2[1], m2[2], m2[3])
    if k1 == "ellipse" and k2 == "line":
        return geommath.line_ellipse(m2[1], m2[2], m1[1], m1[2], m1[3])
    if k1 == "circle" and k2 == "circle":
        return geommath.circle_circle(m1[1], m1[2], m2[1], m2[2])
    if k1 == "circle" and k2 == "arc":
        return [p for p in geommath.circle_circle(m1[1], m1[2], m2[1], m2[2])
                if geommath._on_arc(p, m2[1], m2[2], m2[3], m2[4])]
    if k1 == "arc" and k2 == "circle":
        return [p for p in geommath.circle_circle(m1[1], m1[2], m2[1], m2[2])
                if geommath._on_arc(p, m1[1], m1[2], m1[3], m1[4])]
    if k1 == "arc" and k2 == "arc":
        return geommath.arc_arc(m1[1], m1[2], m1[3], m1[4],
                                m2[1], m2[2], m2[3], m2[4])
    if k1 == "circle" and k2 == "ellipse":
        return circle_ellipse(m1[1], m1[2], m2[1], m2[2], m2[3])
    if k1 == "ellipse" and k2 == "circle":
        return circle_ellipse(m2[1], m2[2], m1[1], m1[2], m1[3])
    if k1 == "ellipse" and k2 == "ellipse":
        return None  # asserted separately (observed kernel behavior)
    return None


def _match_points(got, want, tol):
    if len(got) != len(want):
        return False
    used = [False] * len(got)
    for (wx, wy) in want:
        found = False
        for k, (gx, gy) in enumerate(got):
            if not used[k] and math.hypot(gx - wx, gy - wy) <= tol:
                used[k] = True
                found = True
                break
        if not found:
            return False
    return True


def assert_intersections(cadcli, cmds, tol=1e-4, extra_pairs=None):
    """Run cmds, build expected per-pair intersections from the scene
    model, and compare with the dump (points + indices + total)."""
    res = cadcli.run(cmds).expect_ok()
    model = [_geom_model(d) for d in res.dobjects]
    got = {}
    for (x, y, i, j) in res.intersections:
        got.setdefault((i, j), []).append((x, y))
    expected = {}
    for i in range(len(model)):
        for j in range(i + 1, len(model)):
            pts = _intersect_pair(model[i], model[j])
            if pts is not None and pts:
                expected[(i, j)] = pts
    for pair, pts in (extra_pairs or {}).items():
        expected[pair] = pts
    assert set(got.keys()) == set(expected.keys()), (
        f"pair sets differ: got {sorted(got)} expected {sorted(expected)}")
    for (i, j), want in expected.items():
        assert _match_points(got[(i, j)], want, tol), (
            f"pair ({i},{j}): got {got[(i, j)]} want {want}")
    return res


# -- line-line -------------------------------------------------------------------

def test_line_line_crossing(cadcli):
    assert_intersections(cadcli, [
        "line 0,0 10,0",
        "line 5,-5 5,5",
    ])


def test_line_line_crossing_both_arg_orders(cadcli):
    res1 = assert_intersections(cadcli, [
        "line 0,0 10,0", "line 5,-5 5,5"])
    res2 = assert_intersections(cadcli, [
        "line 5,-5 5,5", "line 0,0 10,0"])
    # the same geometric points, indices swapped
    assert res1.intersections[0][0:2] == res2.intersections[0][0:2]


def test_line_line_parallel(cadcli):
    res = assert_intersections(cadcli, [
        "line 0,0 10,0", "line 0,5 10,5"])
    assert res.intersection_count == 0


def test_line_line_collinear_overlapping(cadcli):
    # overlapping collinear segments report NO intersection (observed kernel
    # behavior — only proper transversal points are reported).
    res = assert_intersections(cadcli, [
        "line 0,0 10,0", "line 2,0 8,0", "line 0,0 5,0"])
    assert res.intersection_count == 0


def test_line_line_endpoint_touching(cadcli):
    assert_intersections(cadcli, [
        "line 0,0 5,5", "line 5,5 10,0"])


def test_line_line_t_junction(cadcli):
    assert_intersections(cadcli, [
        "line 0,0 10,0", "line 5,0 5,5"])


# -- line-circle -----------------------------------------------------------------

def test_line_circle_two_points(cadcli):
    assert_intersections(cadcli, [
        "line 0,0 10,0", "circle 5,0 2"])


def test_line_circle_tangent_one_point(cadcli):
    assert_intersections(cadcli, [
        "line -1,2 11,2", "circle 5,0 2"])


def test_line_circle_miss(cadcli):
    res = assert_intersections(cadcli, [
        "line 0,0 10,0", "circle 5,10 2"])
    assert res.intersection_count == 0


def test_line_circle_line_is_segment_clipped(cadcli):
    # the infinite line would cross the circle at x=14 — outside the segment.
    res = assert_intersections(cadcli, [
        "line 0,0 10,0", "circle 12,0 2"])
    assert res.intersection_count == 1


# -- circle-circle ----------------------------------------------------------------

def test_circle_circle_two_points(cadcli):
    assert_intersections(cadcli, [
        "circle 0,0 5", "circle 8,0 5"])


def test_circle_circle_external_tangent(cadcli):
    assert_intersections(cadcli, [
        "circle 0,0 2", "circle 4,0 2"])


def test_circle_circle_internal_tangent(cadcli):
    assert_intersections(cadcli, [
        "circle 0,0 6", "circle 5,0 1"])


def test_circle_circle_concentric(cadcli):
    res = assert_intersections(cadcli, [
        "circle 0,0 2", "circle 0,0 6"])
    assert res.intersection_count == 0


def test_circle_circle_identical(cadcli):
    # identical circles report 0 (observed kernel behavior).
    res = assert_intersections(cadcli, [
        "circle 0,0 5", "circle 0,0 5", "circle 0,0 5"])
    assert res.intersection_count == 0


# -- line-arc / arc-arc ------------------------------------------------------------

def test_line_arc_two_points(cadcli):
    assert_intersections(cadcli, [
        "line -10,1.5 10,1.5", "arc 0,0 5 0 180"])


def test_line_arc_one_point(cadcli):
    # tangent line touching the arc mid-span (90°) → single point (0,5).
    assert_intersections(cadcli, [
        "line -10,5 10,5", "arc 0,0 5 0 180"])


def test_line_arc_miss(cadcli):
    res = assert_intersections(cadcli, [
        "line -10,6 10,6", "arc 0,0 5 0 180"])
    assert res.intersection_count == 0


def test_arc_arc_two_points(cadcli):
    assert_intersections(cadcli, [
        "arc 0,0 5 0 330", "arc 8,0 5 90 270"])


def test_arc_arc_shared_endpoint_reports_nothing(cadcli):
    # Two half-circles sharing endpoints report 0 (observed kernel behavior —
    # arc-arc endpoint contacts are not intersection points).
    res = assert_intersections(cadcli, [
        "arc 0,0 5 0 180", "arc 0,0 5 180 360"])
    assert res.intersection_count == 0


# -- ellipse combos -----------------------------------------------------------------

def test_ellipse_line(cadcli):
    assert_intersections(cadcli, [
        "ellipse 0,0 5,0 2", "line -10,0 10,0"])


def test_ellipse_circle_axis_aligned(cadcli):
    assert_intersections(cadcli, [
        "ellipse 0,0 5,0 2", "circle 0,0 2"])


def test_ellipse_circle_miss(cadcli):
    res = assert_intersections(cadcli, [
        "ellipse 0,0 5,0 2", "circle 0,0 1"])
    assert res.intersection_count == 0


def test_ellipse_circle_offset(cadcli):
    assert_intersections(cadcli, [
        "ellipse 0,0 5,0 2", "circle 3,0 4"])


def test_ellipse_ellipse_tangent_observed(cadcli):
    # Two identical ellipses 10 apart: single tangent point (5,0) — the
    # observed kernel behavior, asserted as documented.
    res = assert_intersections(cadcli, [
        "ellipse 0,0 5,0 2", "ellipse 10,0 5,0 2"],
        extra_pairs={(0, 1): [(5.0, 0.0)]})
    assert res.intersection_count == 1


def test_intersection_count_matches_total(cadcli):
    res = cadcli.run([
        "line 0,0 10,0", "circle 5,0 2", "circle 0,0 3",
        "arc 5,5 2 0 180", "line 0,10 10,10",
    ]).expect_ok()
    assert len(res.intersections) == res.intersection_count
    for (x, y, i, j) in res.intersections:
        assert i < j
        assert i < len(res.dobjects) and j < len(res.dobjects)
