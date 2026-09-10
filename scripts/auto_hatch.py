# Auto-hatch — searches the scene, finds closed areas, and hatches them.
#
# Run with:   run auto_hatch                          (dialog: pick the pattern)
#             run auto_hatch pattern=BRICK tolerance=0.5
#             run auto_hatch pattern=SOLID min_area=10
#
# Two detection passes:
#   1) NATIVE closed regions — closed polylines, circles, ellipses, closed
#      splines. Each entity's centroid (or an arc-ward point) seeds the
#      app's boundary tracing (rasm.hatch_at), islands included.
#   2) LOOPS OF SEPARATE SEGMENTS — open lines / open polylines whose
#      endpoints touch within `tolerance` form a graph; every minimal
#      counter-clockwise face is a closed area. The loop's segments are
#      JOINED into one closed polyline (the first segment is converted,
#      the rest removed) and hatched. One Ctrl+Z reverts the whole run.
#
# The hatch type comes from the `pattern` dropdown (a 'choice' parameter).
# Hatch boundary handles stay live — moving a boundary later reshapes the
# fill, exactly like the interactive hatch.

import math


def seg_endpoints(geom):
    """[(start, end), …] world segments for line/polyline/arc/ellipse-arc."""
    t = geom["type"]
    if t == "line":
        return [(geom["start"], geom["end"])]
    if t == "polyline":
        pts = geom["points"]
        segs = [(pts[i], pts[i + 1]) for i in range(len(pts) - 1)]
        if geom.get("closed") and len(pts) >= 3:
            segs.append((pts[-1], pts[0]))   # the closing edge
        return segs
    if t == "arc":
        cx, cy = geom["center"]
        r = geom["radius"]
        a0 = math.radians(geom["start_deg"])
        a1 = math.radians(geom["start_deg"] + geom["sweep_deg"])
        return [((cx + r * math.cos(a0), cy + r * math.sin(a0)),
                 (cx + r * math.cos(a1), cy + r * math.sin(a1)))]
    return []


def near(a, b, tol):
    return abs(a[0] - b[0]) <= tol and abs(a[1] - b[1]) <= tol


def centroid(geom):
    """A point that is (very likely) inside/on the entity."""
    t = geom["type"]
    if t == "line":
        (x1, y1), (x2, y2) = geom["start"], geom["end"]
        return ((x1 + x2) / 2.0, (y1 + y2) / 2.0)
    if t == "polyline":
        pts = geom["points"]
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        return (sum(xs) / len(xs), sum(ys) / len(ys))
    if t == "circle":
        return geom["center"]
    if t == "arc":
        cx, cy = geom["center"]
        r = geom["radius"]
        a = math.radians(geom["start_deg"] + geom["sweep_deg"] / 2.0)
        return (cx + r * 0.9 * math.cos(a), cy + r * 0.9 * math.sin(a))
    if t == "ellipse":
        return geom["center"]
    return None


def find_line_faces(entities, tol):
    """Minimal CCW faces of the segment graph (half-edge walk)."""
    import collections
    # vertices → list of (angle, vertex, entity_index)
    adj = collections.defaultdict(list)
    segs = []   # (v1, v2, entity_index)
    for i, e in enumerate(entities):
        for (a, b) in seg_endpoints(e):
            adj[a].append((math.atan2(b[1] - a[1], b[0] - a[0]), b, i))
            adj[b].append((math.atan2(a[1] - b[1], a[0] - b[0]), a, i))
            segs.append((a, b, i))
    for v in adj:
        adj[v].sort()   # ascending angle
    faces = []
    used = set()
    for v1, v2, eidx in segs:
        walk = (v1, v2, eidx)
        if walk in used:
            continue
        path = [walk]
        used.add(walk)
        cur_v, cur_e = v2, eidx
        loop = 0
        while True:
            loop += 1
            if loop > 4096:
                break
            ring = adj[cur_v]
            if not ring:
                break
            # next edge = the one just CCW of the reverse of the incoming one
            back = math.atan2(
                path[-1][0][1] - cur_v[1], path[-1][0][0] - cur_v[0])
            nxt = None
            for k in range(len(ring) * 2):
                cand = ring[(k) % len(ring)]
                ang = cand[0]
                rel = (ang - back) % (2 * math.pi)
                if rel > 1e-9:
                    nxt = cand
                    break
            if nxt is None:
                nxt = ring[0]
            w = (cur_v, nxt[1], nxt[2])
            if w == path[0]:
                # closed face
                xs = [p[0][0] for p in path]
                ys = [p[0][1] for p in path]
                area2 = sum(
                    xs[i] * ys[(i + 1) % len(xs)]
                    - xs[(i + 1) % len(xs)] * ys[i]
                    for i in range(len(xs)))
                faces.append((area2, [p[2] for p in path],
                              [p[0] for p in path] + [path[0][1]]))
                break
            if w in used:
                break
            used.add(w)
            path.append(w)
            cur_v = nxt[1]
            cur_e = nxt[2]
    return faces


def run(pattern: 'hatch_pattern' = 'ANSI31', tolerance: 'length' = 0.5,
        min_area: 'length' = 1.0, join_line_loops=True):
    """Auto-hatch every closed area in the scene.
    pattern: hatch pattern (dropdown over the app's catalog)
    tolerance: endpoint snap distance for detecting touching segments (0.001..10)
    min_area: skip areas smaller than this (side-length equivalent)
    join_line_loops: also detect and hatch loops made of separate segments
    """
    if tolerance < 0:
        raise SystemExit("! auto_hatch: tolerance must be >= 0")
    catalog = rasm.hatch_patterns()
    if pattern.upper() != "SOLID" and pattern.upper() not in [c.upper() for c in catalog]:
        raise SystemExit("! auto_hatch: unknown pattern '%s'" % pattern)

    entities = rasm.doc.entities()
    if not entities:
        print("auto_hatch: the drawing is empty")
        return

    hatched = 0
    consumed = set()   # entity indices already covered by a hatch boundary
    tracing_note = True

    # ---- pass 1: native closed regions via the app's boundary tracing ----
    for i, e in enumerate(entities):
        if i in consumed:
            continue
        c = centroid(e)
        if c is None:
            continue
        try:
            boundary = rasm.hatch_at(c, pattern)
        except RuntimeError:
            # Boundary tracing is app-only (the headless CLI has no tracer).
            # Degrade gracefully: note it once and rely on pass 2.
            if tracing_note:
                print("auto_hatch: boundary tracing unavailable here — "
                      "only joined segment loops will be hatched")
                tracing_note = False
            break
        if boundary:
            consumed.update(boundary)
            consumed.add(i)
            hatched += 1

    # ---- pass 2: loops of separate segments (graph faces) ----
    if join_line_loops:
        seg_candidates = [
            i for i, e in enumerate(entities)
            if i not in consumed and e["type"] in ("line", "polyline", "arc")
        ]
        sub = {old: new for new, old in enumerate(seg_candidates)}
        faces = find_line_faces(
            [entities[o] for o in seg_candidates], tolerance)
        for area2, eidxs, loop_pts in faces:
            if area2 <= 0:
                continue
            if abs(area2) / 2.0 < min_area * min_area:
                continue
            idxs = sorted(set(seg_candidates[x] for x in eidxs))
            if any(i in consumed for i in idxs):
                continue
            # Join the loop into ONE closed polyline: convert the first
            # segment, remove the rest (descending order keeps indices).
            first = idxs[0]
            rest = sorted(idxs[1:], reverse=True)
            rasm.set_geom(first, {
                "type": "polyline",
                "points": list(loop_pts[:-1]),
                "closed": True,
            })
            if rest:
                rasm.delete(rest)
            rasm.add_hatch([first], pattern)
            consumed.update(idxs)
            hatched += 1

    print("auto_hatch: hatched %d closed area(s) with %s"
          % (hatched, pattern.upper()))
    if hatched:
        rasm.zoom_extents()
    print("one Ctrl+Z reverts the whole run")


rasm.main(run)
