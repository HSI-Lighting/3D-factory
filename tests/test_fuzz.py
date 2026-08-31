"""Seeded random rounds (plan §17).

Round 1 — random CLI lines (valid + invalid + interactive-only): exit 0,
no panic output, no tracebacks, every reply matches a known prefix, the
scene dump always parses, dobject count never negative.

Round 2 — random rasm op sequences through pyfile with try/except around
every op (OP-OK/OP-ERR marks): no escaped traceback, and the doc count
recomputed from json read-back after EVERY step matches the Python
simulation.
"""

import json
import random

import pytest

from tests.cadcli import check_obj

SEEDS = [1, 7, 42, 1234, 2024, 31337, 55555, 99991, 123456, 8675309]

REPLY_PREFIXES = ("+ ", "- ", "( ", "(", "commands:", "list ", "run: ",
                  "run> ", "python: ", "python:", "  ", "= ")

CLI_POOL = [
    "line 0,0 10,0", "line -5,5 5,-5", "line a,b 1,1", "line 0,0",
    "circle 0,0 2", "circle 5,5 0.5", "circle 0,0 -1", "circle x,y 3",
    "circle 0,0", "arc 0,0 5 0 90", "arc 0,0 5 350 10", "arc 0,0 0 0 90",
    "arc3p 0,0 5,0 0,5", "arc3p 0,0 1,0 2,0", "arcse 0,0 5,0 0,5",
    "arccr 0,0 10,0 6", "arccl 0,0 10,0 20", "arccl 0,0 10,0 5",
    "ellipse 0,0 5,0 2", "ellipse 0,0 0,0 2", "point 3,4", "point x",
    "polyline 0,0 10,0 10,10 close", "polyline 0,0 10,0 10,10 0,10 5,5",
    "polyline 0,0", "del 0", "del 999", "del -1", "del abc", "clear",
    "list", "ls", "help", "?", "grips", "end", "mid", "per", "tan",
    "move", "copy", "rotate", "scale", "mirror", "hatch ANSI31", "trim",
    "fillet", "offset 2", "undo", "redo", "select", "all", "window",
    "fence", "wp", "last", "unknown-command-xyz", "open f.rsm", "save f",
    "py", "pyfile", "run", "run nosuch", "chprop layer Walls",
    "text hello", "wall 3", "card on", "blockdiff a b", "lengthen 5",
    "brea", "stretch", "extend 5", "join", "dist", "area", "purge",
    "  line 1,1 2,2  ", "# comment", "",
]


def gen_round1(rng, n=40):
    return [rng.choice(CLI_POOL) for _ in range(n)]


@pytest.mark.parametrize("seed", SEEDS)
def test_fuzz_round1_cli_lines(cadcli, seed):
    rng = random.Random(seed)
    lines = gen_round1(rng)
    res = cadcli.run(lines).expect_ok(allow_errors=True)
    assert not res.tracebacks, f"seed {seed} produced tracebacks"
    assert res.exit_code == 0
    assert not res.panicked
    assert res.raw_stderr == ""
    for r in res.replies:
        assert r.startswith(REPLY_PREFIXES), f"unexpected reply {r!r}"
    assert res.intersection_count == len(res.intersections)
    assert res.intersection_count >= 0
    assert len(res.dobjects) >= 0
    # count consistency: dump header matched the parsed dobjects already


# -- round 2: rasm ops via pyfile ----------------------------------------------

FUZZ_OPS = ["add_line", "add_circle_ok", "add_circle_bad", "add_point",
            "add_text", "add_polyline", "delete", "move", "set_color",
            "set_color_bad", "add_layer", "set_layer", "add_hatch",
            "set_visible", "copy"]


def fuzz_op_code(rng, step, kind, kinds):
    """Generate the python for one op.

    Returns (code, expected_ok, sim_kind): sim_kind is the entity kind to
    append to the simulation when the op succeeds ("" = nothing)."""
    if kind == "add_line":
        a = (rng.uniform(-50, 50), rng.uniform(-50, 50))
        b = (rng.uniform(-50, 50), rng.uniform(-50, 50))
        return f"rasm.add_line({a}, {b})", True, "line"
    if kind == "add_circle_ok":
        c = (rng.uniform(-20, 20), rng.uniform(-20, 20))
        return (f"rasm.add_circle({c}, {rng.uniform(0.5, 10)})",
                True, "circle")
    if kind == "add_circle_bad":
        return f"rasm.add_circle((0,0), {rng.uniform(-5, 0)})", False, ""
    if kind == "add_point":
        return (f"rasm.add_point({(rng.uniform(-30, 30), rng.uniform(-30, 30))})",
                True, "point")
    if kind == "add_text":
        return (f"rasm.add_text('f{step}', (0,0), {rng.uniform(0.1, 5)})",
                True, "text")
    if kind == "add_polyline":
        n = rng.randint(2, 5)
        pts = [(rng.uniform(-10, 10), rng.uniform(-10, 10)) for _ in range(n)]
        closed = rng.random() < 0.5
        return (f"rasm.add_polyline({pts}, closed={closed})", True,
                "polyline" if closed else "pline_open")
    if kind == "delete":
        ok = len(kinds) > 0
        return "rasm.delete([0])", ok, ""
    if kind == "move":
        return (f"rasm.move([0], {rng.uniform(-10, 10)}, "
                f"{rng.uniform(-10, 10)})", len(kinds) > 0, "")
    if kind == "set_color":
        return f"rasm.set_color([0], {rng.randint(0, 255)})", len(kinds) > 0, ""
    if kind == "set_color_bad":
        return "rasm.set_color([0], 300)", False, ""
    if kind == "add_layer":
        return f"rasm.add_layer('L{step}')", True, ""
    if kind == "set_layer":
        return f"rasm.set_layer('L{step}')", True, ""
    if kind == "add_hatch":
        closed0 = kinds and kinds[0] in ("circle", "ellipse", "polyline")
        return "rasm.add_hatch([0], 'SOLID')", closed0, "hatch" if closed0 else ""
    if kind == "set_visible":
        return (f"rasm.set_visible([0], {rng.random() < 0.5})",
                len(kinds) > 0, "")
    if kind == "copy":
        return (f"rasm.copy([0], {rng.uniform(-5, 5)}, "
                f"{rng.uniform(-5, 5)})", len(kinds) > 0,
                "")
    raise AssertionError(kind)


def apply_fuzz_op(kinds, kind, expected_ok, sim_kind):
    kinds = list(kinds)
    if not expected_ok:
        return kinds
    if sim_kind:
        kinds.append(sim_kind)
    elif kind in ("delete",):
        kinds.pop(0)
    elif kind == "copy":
        kinds.append(kinds[0])
    return kinds


@pytest.mark.parametrize("seed", SEEDS)
def test_fuzz_round2_rasm_ops(cadcli, seed):
    rng = random.Random(seed)
    kinds = []
    lines = ["print('START')"]
    expected_counts = []
    for step in range(25):
        kind = rng.choice(FUZZ_OPS)
        code, expected_ok, sim_kind = fuzz_op_code(rng, step, kind, kinds)
        lines.append(f"try:\n    {code}\n    print('OP-OK')\n"
                     f"except Exception:\n    print('OP-ERR')")
        lines.append("print('COUNT>' + str(rasm.doc.count()))")
        kinds = apply_fuzz_op(kinds, kind, expected_ok, sim_kind)
        expected_counts.append(len(kinds))
    lines.append("import json; print('END>>' + json.dumps(rasm.doc.count()))")

    res = cadcli.py_file("\n".join(lines)).expect_ok(allow_errors=True)
    assert not res.tracebacks, f"seed {seed}: escaped traceback"
    assert res.exit_code == 0 and not res.panicked

    marks = [r for r in res.replies if r in ("OP-OK", "OP-ERR")]
    assert len(marks) == 25, (
        f"seed {seed}: expected 25 op marks, got {len(marks)}")

    counts = [int(r[len("COUNT>"):]) for r in res.replies
              if r.startswith("COUNT>")]
    assert len(counts) == 25
    assert counts == expected_counts, (
        f"seed {seed}: per-step count mismatch\n"
        f"got {counts}\nwant {expected_counts}\n{res.raw_stdout}")

    end = [r for r in res.replies if r.startswith("END>>")]
    assert end, res.replies
    final = int(json.loads(end[0][len("END>>"):]))
    assert final == len(res.dobjects), (
        f"seed {seed}: json {final} != dump {len(res.dobjects)}")
    assert final == len(kinds), f"seed {seed}: json {final} != sim {len(kinds)}"
    assert final >= 0
