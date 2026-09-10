"""Every script in scripts/ — run forms, param grids, validation, scene
effects, headless-specific behaviors and idempotency.

Each test runs in a fresh process so layer/block state never leaks between
runs (plan §15).
"""

import json
import re

import pytest

from tests.cadcli import check_obj

SCRIPT_NAMES = ["auto_hatch", "block_demo", "flange", "grid_circles",
                "hello", "layers_demo", "modify_demo"]

_MARK = "JSON>>"


def _run(cadcli, args: str = ""):
    # script runs legitimately produce metadata-pass tracebacks (hello,
    # block_demo) and validation errors — assert them per-test instead.
    return cadcli.run([f"run {args}".strip()]).expect_ok(
        allow_errors=True, allow_tracebacks=True)


def _with_json(res, expr):
    """Batch a json read-back into the SAME process as the script run."""
    return res.client.run(res.script + [
        f"py import json; print('{_MARK}' + json.dumps({expr}))"])


def _jline(res):
    for line in res.replies:
        if line.startswith(_MARK):
            return json.loads(line[len(_MARK):])
    raise AssertionError(f"no {_MARK} line in replies: {res.replies}")


def test_bare_run_lists_scripts(cadcli):
    res = cadcli.run(["run"]).expect_ok()
    listed = [r for r in res.replies if r.startswith("run: scripts available:")]
    assert listed, res.replies
    for name in SCRIPT_NAMES:
        assert name in listed[0], f"{name} missing from {listed[0]}"


def test_run_unknown_script(cadcli):
    res = _run(cadcli, "nosuch")
    res.expect_error_containing(
        "! run: no script 'nosuch' in scripts/ (available:")
    assert res.dobjects == []


def test_run_case_insensitive_resolution(cadcli):
    res = _run(cadcli, "HELLO")
    assert any(r.startswith("run> hello") for r in res.replies)
    assert len(res.dobjects) == 2


# -- hello -------------------------------------------------------------------------

def test_run_hello(cadcli):
    res = _run(cadcli, "hello")
    assert any(r == "hello from python" for r in res.replies)
    assert any(r == "doc now has 2 entities" for r in res.replies)
    assert len(res.dobjects) == 2
    check_obj(res.dobjects[0], "circle", center=(0, 0), radius=5.0)
    check_obj(res.dobjects[1], "line", a=(0, 0), b=(5, 5))
    # DOCUMENTED: the metadata pass runs the module top-level, where
    # hello.py reads rasm.doc.get() — the meta pass answers with an error,
    # producing a traceback before the real run succeeds.
    res.expect_traceback_containing(
        "RuntimeError: parameter scan does not read the document")


# -- grid_circles -------------------------------------------------------------------

def test_run_grid_circles_defaults(cadcli):
    res = _run(cadcli, "grid_circles")
    assert len(res.dobjects) == 48  # 8 x 6 default
    assert any(r == "drawing a 8x6 grid of circles (spacing 10.0)"
               for r in res.replies)


def test_run_grid_circles_positional(cadcli):
    res = _run(cadcli, "grid_circles 3 2 5")
    assert len(res.dobjects) == 6
    assert any(r == "drawing a 3x2 grid of circles (spacing 5.0)"
               for r in res.replies)
    centers = [d["center"] for d in res.dobjects]
    assert (0.0, -2.5) in centers
    assert (5.0, 2.5) in centers
    assert (0.0, 0.0) not in centers


def test_run_grid_circles_named_params_ignored_positional(cadcli):
    # grid_circles reads rasm.args only; named params fall back to defaults
    res = _run(cadcli, "grid_circles cols=3 rows=2")
    assert len(res.dobjects) == 48


# -- flange --------------------------------------------------------------------------

def test_run_flange_defaults(cadcli):
    res = _run(cadcli, "flange")
    assert len(res.dobjects) == 10  # 2 body + 6 bolts + 2 axes
    check_obj(res.dobjects[0], "circle", center=(0, 0), radius=60.0)
    check_obj(res.dobjects[1], "circle", center=(0, 0), radius=30.0)


def test_run_flange_named_length_scene_units(cadcli):
    res = _run(cadcli, "flange outer_d=25 bore_d=10 hole_d=1")
    assert len(res.dobjects) == 10
    check_obj(res.dobjects[0], "circle", center=(0, 0), radius=12.5)


def test_run_flange_named_with_unit_suffix(cadcli):
    # 25cm → 250 scene units (doc unit is mm, scene_per_unit 1.0)
    res = _run(cadcli, "flange outer_d=25cm bore_d=10 hole_d=1")
    check_obj(res.dobjects[0], "circle", radius=125.0)
    res = _run(cadcli, 'flange outer_d=6" bore_d=2 hole_d=0.5')
    check_obj(res.dobjects[0], "circle", radius=76.2)
    res = _run(cadcli, "flange outer_d=6' bore_d=6 hole_d=1")
    check_obj(res.dobjects[0], "circle", radius=914.4)


def test_run_flange_positional_form(cadcli):
    res = _run(cadcli, "flange 150 70 10")
    assert len(res.dobjects) == 14  # 10 bolts + 2 + 2
    check_obj(res.dobjects[0], "circle", radius=75.0)


def test_run_flange_layer_and_color(cadcli):
    res = _with_json(_run(cadcli, "flange outer_d=25 bore_d=6 hole_d=1"),
                     "[l for l in rasm.doc.layers()]")
    layers = {l["name"]: l for l in _jline(res)}
    assert "flange_holes" in layers
    assert "Aci(5)" in layers["flange_holes"]["color"]


def test_run_flange_invalid_named_length(cadcli):
    res = _run(cadcli, "flange outer_d=abc")
    res.expect_error_containing(
        "! run flange: outer_d: 'abc' is not a valid length")
    assert res.dobjects == []


def test_run_flange_validation_systemexit(cadcli):
    res = _run(cadcli, "flange bore_d=200")
    res.expect_traceback_containing("! flange: bore_d must be smaller than outer_d")
    assert res.dobjects == []
    res = _run(cadcli, "flange bolts=2")
    res.expect_traceback_containing("! flange: at least 3 bolts")


def test_run_flange_unknown_named_param_passthrough(cadcli):
    # an undeclared k=v is passed through raw and simply unused
    res = _run(cadcli, "flange outer_d=25 bore_d=10 hole_d=1 nonsense=1")
    assert len(res.dobjects) == 10


# -- layers_demo ----------------------------------------------------------------------

def test_run_layers_demo(cadcli):
    res = _with_json(
        _run(cadcli, "layers_demo"),
        "[[l['name'] for l in rasm.doc.layers()], rasm.doc.active_layer(), "
        "[l['color'] for l in rasm.doc.layers()]]")
    names, active, colors = _jline(res)
    assert "walls" in names and "doors" in names
    assert len(res.dobjects) == 5  # 4 lines + 1 arc
    assert active == 2  # doors is the second created layer
    walls_color = colors[names.index("walls")]
    assert "Aci(5)" in walls_color


def test_run_layers_demo_twice_idempotency(cadcli):
    # DOCUMENTED: layers_demo has no guard; the second run fails loudly on
    # the duplicate layer (no crash, exit 0, document unchanged).
    # Both runs must share ONE process to see the duplicate.
    res = cadcli.run(["run layers_demo", "run layers_demo"]).expect_ok(
        allow_errors=True, allow_tracebacks=True)
    assert len(res.dobjects) == 5
    res.expect_traceback_containing(
        "RuntimeError: layer 'walls' already exists")


# -- modify_demo ----------------------------------------------------------------------

def test_run_modify_demo(cadcli):
    # modify_demo.py exercises the full modify surface: style setters,
    # transforms, set_geom edits, a handle-based lookup and a rejected edit.
    res = _run(cadcli, "modify_demo")
    assert not res.tracebacks, res.tracebacks
    assert any(r.startswith("hole at (20.0, 10.0) radius 5.0 color aci 7")
               for r in res.replies)
    assert any(r.startswith("hole now at (70.0, 10.0) radius 6.0")
               for r in res.replies)
    assert any(r.startswith("expected rejection:") for r in res.replies)
    assert any(r.startswith("duplicated hole ->") for r in res.replies)
    assert len(res.dobjects) == 3  # base polyline + hole + duplicate


# -- auto_hatch -----------------------------------------------------------------------

def test_run_auto_hatch_empty_scene(cadcli):
    res = _run(cadcli, "auto_hatch pattern=ANSI31")
    assert any(r == "auto_hatch: the drawing is empty" for r in res.replies)
    assert res.dobjects == []


def test_run_auto_hatch_closed_loop(cadcli):
    # A square made of 4 separate lines: pass 2 joins the loop into one
    # closed polyline and hatches it (headless has no tracer — documented).
    res = cadcli.run([
        "line 0,0 10,0", "line 10,0 10,10", "line 10,10 0,10",
        "line 0,10 0,0",
        "run auto_hatch pattern=ANSI31",
    ]).expect_ok()
    assert any(r.startswith("auto_hatch: boundary tracing unavailable")
               for r in res.replies)
    assert any(r == "auto_hatch: hatched 1 closed area(s) with ANSI31"
               for r in res.replies)
    # 4 lines → joined polyline (3 deleted) + 1 hatch
    assert len(res.dobjects) == 2
    assert res.dobjects[0].kind == "polyline"
    assert res.dobjects[0]["closed"] is True
    assert res.dobjects[0]["verts"] == 4
    check_obj(res.dobjects[1], "hatch", boundary_loops=1)
    assert "ANSI31" in res.dobjects[1]["pattern"]


def test_run_auto_hatch_invalid_pattern_catalog(cadcli):
    res = _run(cadcli, "auto_hatch pattern=BOGUS")
    res.expect_error_containing(
        "! run auto_hatch: pattern: 'BOGUS' is not one of [SOLID, ANSI31")


def test_run_auto_hatch_invalid_tolerance_length(cadcli):
    res = _run(cadcli, "auto_hatch tolerance=abc")
    res.expect_error_containing(
        "! run auto_hatch: tolerance: 'abc' is not a valid length")


def test_run_auto_hatch_solid(cadcli):
    res = cadcli.run([
        "line 0,0 10,0", "line 10,0 10,10", "line 10,10 0,10",
        "line 0,10 0,0",
        "run auto_hatch pattern=SOLID",
    ]).expect_ok()
    assert any(r == "auto_hatch: hatched 1 closed area(s) with SOLID"
               for r in res.replies)
    check_obj(res.dobjects[1], "hatch", boundary_loops=1)
    assert "Solid" in res.dobjects[1]["pattern"]


# -- block_demo -----------------------------------------------------------------------

def test_run_block_demo_documents_headless_blocks(cadcli):
    # DOCUMENTED: block ops answer loudly headless. The 5 square lines are
    # drawn; create_block then raises "blocks are not available in the
    # headless CLI" (loud, not silent).
    res = _run(cadcli, "block_demo")
    assert len(res.dobjects) == 5
    res.expect_traceback_containing(
        "RuntimeError: blocks are not available in the headless CLI")
    assert not any("has no attribute" in t for t in res.tracebacks)


def test_run_scripts_never_panic(cadcli):
    for name in SCRIPT_NAMES:
        res = _run(cadcli, name)
        assert not res.panicked, f"{name} panicked:\n{res.raw_stdout}"
