"""Ordered command-pair sequences over a curated alphabet (plan §16).

Every scenario runs in its own process with a fixed base document. Each op
has a deterministic effect on the entity list, simulated in Python; the
final `rasm.doc.count()` read-back, the scene dump and (for triples) the
document bounds must all match the simulation exactly.
"""

import itertools
import json

import pytest

from tests.cadcli import check_obj

# op name → CLI line (run in the SAME process as the base setup)
OPS = {
    "line": "line 0,0 10,0",
    "circle": "circle 0,0 2",
    "arc3p": "arc3p 0,0 5,0 0,5",
    "ellipse": "ellipse 0,0 5,0 2",
    "polyline-close": "polyline 0,0 10,0 10,10 0,10 close",
    "point": "point 3,4",
    "del": "del 0",
    "clear": "clear",
    "rasm-add": "py rasm.add_circle((1,1), 1)",
    "rasm-move": "py rasm.move([0], 1, 1)",
    "rasm-copy": "py rasm.copy([0], 1, 1)",
    "rasm-rotate": "py rasm.rotate([0], (0,0), 45)",
    "rasm-scale": "py rasm.scale([0], (0,0), 2)",
    "rasm-mirror": "py rasm.mirror([0], (0,0), (1,0))",
    "rasm-set_color": "py rasm.set_color([0], 3)",
    "rasm-set_layer": "py rasm.set_layer('seq')",
    "rasm-add_layer": "py rasm.add_layer('seq2')",
    "rasm-add_hatch": "py rasm.add_hatch([0], 'SOLID')",
    "rasm-save": "py rasm.save('<TMP>/seq.rsm')",
    "rasm-delete": "py rasm.delete([0])",
    "py-read": "py rasm.doc.count()",
}

# geometry op → entity kind appended
ADD_KINDS = {
    "line": "line", "circle": "circle", "arc3p": "arc",
    "ellipse": "ellipse", "polyline-close": "polyline", "point": "point",
}

BASE = ["circle 0,0 2", "line 0,0 10,0", "py rasm.add_layer('seq')"]

CIRCLE_BOX = ((-2.0, -2.0), (2.0, 2.0))    # circle 0,0 r2
LINE_BOX = ((0.0, 0.0), (10.0, 0.0))        # line 0,0 -> 10,0

# base entities as (kind, bbox) — bbox is exact only for line/circle, which
# is all the triple chains add
BASE_ENTS = [("circle", CIRCLE_BOX), ("line", LINE_BOX)]


def _translate(box, dx, dy):
    return ((box[0][0] + dx, box[0][1] + dy),
            (box[1][0] + dx, box[1][1] + dy))

MARK = "BOUNDS>>"


def apply_op(ents, name):
    """Simulate one op on the entity list [(kind, bbox)]. Returns (ents, ok)."""
    ents = list(ents)
    if name in ADD_KINDS:
        box = CIRCLE_BOX if name == "circle" else               LINE_BOX if name == "line" else None
        ents.append((ADD_KINDS[name], box))
    elif name == "del":
        if not ents:
            return ents, False
        ents.pop(0)
    elif name == "clear":
        ents = []
    elif name == "rasm-add":
        ents.append(("circle", CIRCLE_BOX))
    elif name == "rasm-copy":
        if not ents:
            return ents, False
        kind, box = ents[0]
        ents.append((kind, _translate(box, 1, 1) if box else None))
    elif name in ("rasm-move", "rasm-rotate", "rasm-scale", "rasm-mirror",
                  "rasm-set_color"):
        if not ents:
            return ents, False
    elif name == "rasm-add_layer":
        pass  # dups traceback, count unchanged
    elif name == "rasm-add_hatch":
        if ents and ents[0][0] in ("circle", "ellipse", "polyline"):
            ents.append(("hatch", None))  # hatches excluded from bounds
        else:
            return ents, False
    elif name == "rasm-save":
        pass
    elif name == "rasm-delete":
        if ents:
            ents.pop(0)  # never errors: Ok(n) with n=0
    elif name == "py-read":
        pass
    return ents, True


def _last_value(res):
    vals = [r[2:] for r in res.replies if r.startswith("= ")]
    assert vals, f"no value reply: {res.errors} {res.tracebacks}"
    return vals[-1]


ALPHABET = list(OPS)


@pytest.mark.parametrize("a,b", list(itertools.product(ALPHABET, repeat=2)),
                         ids=lambda x: x)
def test_pair_sequences(cadcli, tmp_path, a, b):
    ents = BASE_ENTS
    for op in (a, b):
        ents, _ = apply_op(ents, op)
    kinds = [k for k, _ in ents]
    lines = BASE + [OPS[a].replace("<TMP>", str(tmp_path)),
                    OPS[b].replace("<TMP>", str(tmp_path)),
                    "py rasm.doc.count()"]
    res = cadcli.run(lines).expect_ok(allow_errors=True,
                                      allow_tracebacks=True)
    assert int(_last_value(res)) == len(kinds), (
        f"pairs {a},{b}: count {_last_value(res)} != {len(kinds)}\n"
        f"{res.raw_stdout}")
    assert len(res.dobjects) == len(kinds), (
        f"pairs {a},{b}: dump {len(res.dobjects)} != {len(kinds)}")


# -- length-3 chains over a reduced subset -----------------------------------------

def _bboxes(ents):
    """(min, max) over entities with a known bbox (hatches/others excluded,
    matching the kernel's DocBounds which skips hatches)."""
    lo = [None, None]
    hi = [None, None]
    for kind, box in ents:
        if box is None:
            continue
        for axis in range(2):
            lo[axis] = box[0][axis] if lo[axis] is None                 else min(lo[axis], box[0][axis])
            hi[axis] = box[1][axis] if hi[axis] is None                 else max(hi[axis], box[1][axis])
    if lo[0] is None:
        return None
    return (tuple(lo), tuple(hi))


TRIPLE_OPS = ["line", "circle", "del", "clear", "rasm-copy", "rasm-add_hatch"]


@pytest.mark.parametrize("chain", list(itertools.product(TRIPLE_OPS, repeat=3)),
                         ids=lambda x: x)
def test_triple_chains_invariants(cadcli, tmp_path, chain):
    ents = BASE_ENTS
    for op in chain:
        ents, _ = apply_op(ents, op)
    kinds = [k for k, _ in ents]
    expected_bounds = _bboxes(ents)
    lines = BASE + [OPS[o].replace("<TMP>", str(tmp_path)) for o in chain] + [
        "py rasm.doc.count()",
        f"py import json; print('{MARK}' + json.dumps(rasm.doc.bounds()))",
    ]
    res = cadcli.run(lines).expect_ok(allow_errors=True,
                                      allow_tracebacks=True)
    assert int(_last_value(res)) == len(kinds), (
        f"chain {chain}: count != {len(kinds)}\n{res.raw_stdout}")
    assert len(res.dobjects) == len(kinds)
    bounds_line = [r for r in res.replies if r.startswith(MARK)]
    got_bounds = json.loads(bounds_line[0][len(MARK):]) if bounds_line else None
    if expected_bounds is None:
        assert got_bounds is None, (
            f"chain {chain}: expected no bounds, got {got_bounds}")
    else:
        assert got_bounds is not None and \
            got_bounds["min"] == list(expected_bounds[0]) and \
            got_bounds["max"] == list(expected_bounds[1]), (
            f"chain {chain}: bounds {got_bounds} != {expected_bounds}\n"
            f"{res.raw_stdout}")
