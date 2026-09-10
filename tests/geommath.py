"""Python ports of the kernel's construction math (cad_kernel/src/construct.rs
and parser.rs) used to compute EXPECTED values for scene-dump assertions.

These are independent implementations, not copies of test output — they
verify the CLI's geometry math against textbook formulas.
"""

from __future__ import annotations

import math

EPS = 1e-9
TAU = 2.0 * math.pi


def norm_angle(a: float) -> float:
    """math.rs norm_angle — rem_euclid(TAU)."""
    return a % TAU


def arc_center_radius_deg(cx, cy, r, start_deg, end_deg):
    """`arc cx,cy r start end` (parser.rs parse_arc)."""
    sweep_deg = (end_deg - start_deg) % 360.0
    if sweep_deg < 1e-6:
        sweep_deg = 360.0
    start_rad = math.radians(start_deg) % TAU
    return (cx, cy, r, math.degrees(start_rad), sweep_deg)


def arc_center_start_end(cx, cy, sx, sy, ex, ey):
    """`arcse` (construct.rs arc_center_start_end)."""
    radius = math.hypot(sx - cx, sy - cy)
    if radius < EPS:
        return None
    start_angle = norm_angle(math.atan2(sy - cy, sx - cx))
    end_angle = math.atan2(ey - cy, ex - cx)
    sweep_raw = norm_angle(end_angle - start_angle)
    sweep = TAU if sweep_raw < EPS else sweep_raw
    return (cx, cy, radius, math.degrees(start_angle), math.degrees(sweep))


def arc_three_points(p1, p2, p3):
    """`arc3p` (construct.rs arc_three_points)."""
    (x1, y1), (x2, y2), (x3, y3) = p1, p2, p3
    d = 2.0 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2))
    e1x, e1y = x2 - x1, y2 - y1
    e2x, e2y = x3 - x1, y3 - y1
    if d * d <= (1e-9 ** 2) * (e1x * e1x + e1y * e1y) * (e2x * e2x + e2y * e2y):
        return None
    p1_sq = x1 * x1 + y1 * y1
    p2_sq = x2 * x2 + y2 * y2
    p3_sq = x3 * x3 + y3 * y3
    ux = (p1_sq * (y2 - y3) + p2_sq * (y3 - y1) + p3_sq * (y1 - y2)) / d
    uy = (p1_sq * (x3 - x2) + p2_sq * (x1 - x3) + p3_sq * (x2 - x1)) / d
    radius = math.hypot(x1 - ux, y1 - uy)
    a1 = math.atan2(y1 - uy, x1 - ux)
    a2 = math.atan2(y2 - uy, x2 - ux)
    a3 = math.atan2(y3 - uy, x3 - ux)
    ccw_total = norm_angle(a3 - a1)
    ccw_to_mid = norm_angle(a2 - a1)
    if ccw_to_mid <= ccw_total + EPS:
        sweep = TAU if ccw_total < EPS else ccw_total
        return (ux, uy, radius, math.degrees(norm_angle(a1)),
                math.degrees(sweep))
    return (ux, uy, radius, math.degrees(norm_angle(a3)),
            math.degrees(TAU - ccw_total))


def _arc_from_center(s, e, center):
    sx, sy = s
    ex, ey = e
    cx, cy = center
    sa = math.atan2(sy - cy, sx - cx)
    ea = math.atan2(ey - cy, ex - cx)
    sweep = norm_angle(ea - sa)
    if sweep < EPS:
        sweep = TAU
    radius = math.hypot(sx - cx, sy - cy)
    return (cx, cy, radius, math.degrees(norm_angle(sa)), math.degrees(sweep))


def arc_chord_radius(s, e, radius, major):
    """`arccr` (construct.rs arc_chord_radius). Returns None when invalid."""
    if radius < EPS:
        return None
    chord_len = math.hypot(e[0] - s[0], e[1] - s[1])
    if chord_len < EPS:
        return None
    half = chord_len * 0.5
    if half > radius + EPS:
        return None
    h = math.sqrt(max(0.0, radius * radius - half * half))
    mid = ((s[0] + e[0]) * 0.5, (s[1] + e[1]) * 0.5)
    # Vec2::perp of normalized chord = (-y, x)
    nx, ny = -(e[1] - s[1]) / chord_len, (e[0] - s[0]) / chord_len
    arc_a = _arc_from_center(s, e, (mid[0] + nx * h, mid[1] + ny * h))
    arc_b = _arc_from_center(s, e, (mid[0] - nx * h, mid[1] - ny * h))
    a_major = arc_a[4] > 180.0
    return arc_a if a_major == major else arc_b


def arc_chord_length(s, e, arc_length, flip):
    """`arccl` (construct.rs arc_chord_length) — bisection solver."""
    if arc_length < EPS:
        return None
    chord_len = math.hypot(e[0] - s[0], e[1] - s[1])
    if chord_len < EPS:
        return None
    if chord_len > arc_length + EPS:
        return None
    ratio = chord_len / arc_length

    def f(theta):
        x = theta * 0.5
        if x < EPS:
            return 1.0 - ratio
        return math.sin(x) / x - ratio

    lo, hi = 1e-9, TAU - 1e-9
    if f(lo) <= 0.0:
        return None
    for _ in range(100):
        mid = 0.5 * (lo + hi)
        if f(mid) > 0.0:
            lo = mid
        else:
            hi = mid
        if (hi - lo) < 1e-13:
            break
    theta = 0.5 * (lo + hi)
    if theta < 1e-6:
        return None
    radius = arc_length / theta
    want_major = theta > math.pi
    return arc_chord_radius(s, e, radius, want_major ^ flip)


def ellipse_center_major_minor(center, major_end, semi_minor):
    """`ellipse` (construct.rs ellipse_center_major_minor). None = degenerate."""
    mx, my = major_end[0] - center[0], major_end[1] - center[1]
    a = math.hypot(mx, my)
    if a < EPS or semi_minor < EPS:
        return None
    return (center[0], center[1], mx, my, min(semi_minor / a, 1.0))


# ---------------------------------------------------------------------------
# intersection math (for test_geometry_intersections expected values)
# ---------------------------------------------------------------------------

def line_line(p1, p2, q1, q2):
    """Intersection of two segments. Returns None when no single point."""
    (x1, y1), (x2, y2) = p1, p2
    (x3, y3), (x4, y4) = q1, q2
    denom = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4)
    if abs(denom) < 1e-12:
        return None
    t = ((x1 - x3) * (y3 - y4) - (y1 - y3) * (x3 - x4)) / denom
    u = -((x1 - x2) * (y1 - y3) - (y1 - y2) * (x1 - x3)) / denom
    if not (-1e-9 <= t <= 1 + 1e-9 and -1e-9 <= u <= 1 + 1e-9):
        return None
    return (x1 + t * (x2 - x1), y1 + t * (y2 - y1))


def line_circle(p1, p2, center, r):
    """Line SEGMENT ∩ circle → list of points (kernel clips to the segment)."""
    cx, cy = center
    dx, dy = p2[0] - p1[0], p2[1] - p1[1]
    fx, fy = p1[0] - cx, p1[1] - cy
    a = dx * dx + dy * dy
    if a < 1e-15:
        return []
    b = 2 * (fx * dx + fy * dy)
    c = fx * fx + fy * fy - r * r
    disc = b * b - 4 * a * c
    if disc < -1e-9:
        return []
    if disc < 0:
        disc = 0.0
    sq = math.sqrt(disc)
    out = []
    for t in ((-b - sq) / (2 * a), (-b + sq) / (2 * a)):
        if -1e-9 <= t <= 1 + 1e-9:
            p = (p1[0] + t * dx, p1[1] + t * dy)
            if not any(math.hypot(p[0] - q[0], p[1] - q[1]) < 1e-9
                       for q in out):
                out.append(p)
    return out


def circle_circle(c1, r1, c2, r2):
    """Full circles ∩ → list of points."""
    (x1, y1), (x2, y2) = c1, c2
    d = math.hypot(x2 - x1, y2 - y1)
    if d < 1e-12 or d > r1 + r2 + 1e-9 or d < abs(r1 - r2) - 1e-9:
        return []
    a = (r1 * r1 - r2 * r2 + d * d) / (2 * d)
    h2 = r1 * r1 - a * a
    if h2 < -1e-9:
        return []
    if h2 < 0:
        h2 = 0.0
    h = math.sqrt(h2)
    xm = x1 + a * (x2 - x1) / d
    ym = y1 + a * (y2 - y1) / d
    if h < 1e-12:
        return [(xm, ym)]
    ox, oy = -h * (y2 - y1) / d, h * (x2 - x1) / d
    return [(xm + ox, ym + oy), (xm - ox, ym - oy)]


def _on_arc(p, center, r, start_deg, sweep_deg):
    """Is point p within the swept CCW arc (inclusive endpoints)?"""
    px, py = p
    cx, cy = center
    ang = math.degrees(math.atan2(py - cy, px - cx)) % 360.0
    s = start_deg % 360.0
    delta = (ang - s) % 360.0
    return delta <= sweep_deg + 1e-6 and delta >= -1e-6


def line_arc(p1, p2, center, r, start_deg, sweep_deg):
    return [p for p in line_circle(p1, p2, center, r)
            if _on_arc(p, center, r, start_deg, sweep_deg)]


def arc_arc(c1, r1, s1, w1, c2, r2, s2, w2):
    pts = circle_circle(c1, r1, c2, r2)
    return [p for p in pts
            if _on_arc(p, c1, r1, s1, w1) and _on_arc(p, c2, r2, s2, w2)]


def line_ellipse(p1, p2, center, major, ratio):
    """Line ∩ axis-parameterised ellipse: p(t)=c+u·cos t+v·sin t, u=major,
    v=perp(major)·ratio. Solves a·cos t + b·sin t = c."""
    (cx, cy) = center
    (ux, uy) = major
    a_len = math.hypot(ux, uy)
    if a_len < EPS:
        return []
    vx, vy = -uy / a_len * a_len * ratio, ux / a_len * a_len * ratio
    dx, dy = p2[0] - p1[0], p2[1] - p1[1]
    fx, fy = p1[0] - cx, p1[1] - cy
    # line: f + t·d = c + u·cos + v·sin  →  cross(d, f - c) + t·cross(d, d)=0 …
    # solve via: (f - c) × d = (u × d)·cos + (v × d)·sin  where × is 2D cross.
    cross = lambda ax, ay, bx, by: ax * by - ay * bx
    rhs = cross(fx, fy, dx, dy)
    au = cross(ux, uy, dx, dy)
    av = cross(vx, vy, dx, dy)
    denom = math.hypot(au, av)
    if denom < 1e-12:
        return []
    k = rhs / denom
    if abs(k) > 1 + 1e-9:
        return []
    # au·cos + av·sin = rhs → R·cos(t - phi) = rhs with R=denom
    phi = math.atan2(av, au)
    t0 = phi + math.acos(max(-1.0, min(1.0, k)))
    t1 = phi - math.acos(max(-1.0, min(1.0, k)))
    out = []
    for t in (t0, t1):
        pt = (cx + ux * math.cos(t) + vx * math.sin(t),
              cy + uy * math.cos(t) + vy * math.sin(t))
        out.append(pt)
    return out


def ellipse_bbox(center, major, ratio):
    """Axis-aligned bbox of a rotated ellipse (inclusive)."""
    cx, cy = center
    ux, uy = major
    a = math.hypot(ux, uy)
    if a < EPS:
        return ((cx, cy), (cx, cy))
    b = a * ratio
    ang = math.atan2(uy, ux)
    dx = math.sqrt((a * math.cos(ang)) ** 2 + (b * math.sin(ang)) ** 2)
    dy = math.sqrt((a * math.sin(ang)) ** 2 + (b * math.cos(ang)) ** 2)
    return ((cx - dx, cy - dy), (cx + dx, cy + dy))
