"""rasm layer API: add_layer / set_layer / layer_set grids, read-back, and
active-layer placement of new entities."""

import itertools

import pytest

from tests.cadcli import check_obj, jrun


def layers_after(cadcli, ops):
    """Run ops (py lines) and read the layer table back, one process."""
    _, (layers,) = jrun(cadcli, ops, "rasm.doc.layers()")
    return {l["name"]: l for l in layers}


def test_default_layer_single(cadcli):
    _, (layers, active) = jrun(cadcli, [],
                               "rasm.doc.layers()", "rasm.doc.active_layer()")
    assert len(layers) == 1
    l = layers[0]
    assert l["id"] == 0
    assert l["name"]
    assert l["visible"] is True
    assert l["locked"] is False
    assert l["frozen"] is False
    assert l["plottable"] is True
    assert "Aci(7)" in l["color"]
    assert active == 0


def test_add_layer_returns_id_and_activates_by_default(cadcli):
    res, (layers, active) = jrun(cadcli, ["py rasm.add_layer('walls')"],
                                 "rasm.doc.layers()", "rasm.doc.active_layer()")
    assert any(r == "= 1" for r in res.replies)
    assert active == 1
    assert layers[1]["name"] == "walls"


def test_add_layer_set_current_false(cadcli):
    _, (active,) = jrun(cadcli, ["py rasm.add_layer('walls', set_current=False)"],
                        "rasm.doc.active_layer()")
    assert active == 0


def test_add_layer_whitespace_trimmed(cadcli):
    layers = layers_after(cadcli, ["py rasm.add_layer('  padded  ')"])
    assert "padded" in layers
    assert "  padded  " not in layers


def test_add_layer_unicode(cadcli):
    layers = layers_after(cadcli, [
        "py rasm.add_layer('مخطط')",
        "py rasm.add_layer('Wände-Übung')",
    ])
    assert {"مخطط", "Wände-Übung"} <= set(layers)


def test_add_layer_empty_name(cadcli):
    res = cadcli.run(["py rasm.add_layer('')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: layer name cannot be empty")
    res = cadcli.run(["py rasm.add_layer('   ')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: layer name cannot be empty")


def test_add_layer_duplicate(cadcli):
    res = cadcli.run(["py rasm.add_layer('walls')",
                      "py rasm.add_layer('walls')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: layer 'walls' already exists")


def test_add_layer_duplicate_case_insensitive(cadcli):
    res = cadcli.run(["py rasm.add_layer('Walls')",
                      "py rasm.add_layer('walls')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: layer 'walls' already exists")


def test_set_layer_valid_and_active_changes(cadcli):
    _, (layers, active) = jrun(cadcli, [
        "py rasm.add_layer('a', set_current=False)",
        "py rasm.add_layer('b', set_current=False)",
        "py rasm.set_layer('a')",
    ], "rasm.doc.layers()", "rasm.doc.active_layer()")
    by_name = {l["name"]: l for l in layers}
    assert active == by_name["a"]["id"]


def test_set_layer_missing(cadcli):
    res = cadcli.run(["py rasm.set_layer('nope')"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no layer named 'nope'")


# -- layer_set flag permutations: 3^4 = 81 -----------------------------------------

def _layer_set_cases():
    for combo in itertools.product([True, False, None], repeat=4):
        flags = dict(zip(("visible", "locked", "frozen", "plottable"), combo))
        yield pytest.param(flags, id="_".join(
            f"{k}={v}" for k, v in flags.items()))


@pytest.mark.parametrize("flags", list(_layer_set_cases()))
def test_layer_set_flag_permutations(cadcli, flags):
    kwargs = ", ".join(f"{k}={v}" for k, v in flags.items())
    layers = layers_after(cadcli, [
        "py rasm.add_layer('T', set_current=False)",
        f"py rasm.layer_set('T', {kwargs})",
    ])
    layer = layers["T"]
    for k, v in flags.items():
        if v is not None:
            assert layer[k] is v, f"{k}: expected {v}, got {layer[k]}"


@pytest.mark.parametrize("aci", [0, 1, 7, 255, 5, 42])
def test_layer_set_color_grid(cadcli, aci):
    layers = layers_after(cadcli, [
        "py rasm.add_layer('C', set_current=False)",
        f"py rasm.layer_set('C', color={aci})",
    ])
    assert f"Aci({aci})" in layers["C"]["color"]


def test_layer_set_missing_layer(cadcli):
    res = cadcli.run(["py rasm.layer_set('nope', visible=False)"]).expect_ok(
        allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: no layer named 'nope'")


def test_layer_set_partial_update_keeps_others(cadcli):
    layers = layers_after(cadcli, [
        "py rasm.add_layer('P', set_current=False)",
        "py rasm.layer_set('P', visible=False)",
    ])
    layer = layers["P"]
    assert layer["visible"] is False
    assert layer["locked"] is False
    assert layer["frozen"] is False
    assert layer["plottable"] is True


def test_new_entities_land_on_active_layer(cadcli):
    # docs §4.1: "New entities go to the currently active layer"
    _, (layers, active, d0, d1) = jrun(cadcli, [
        "py rasm.add_layer('walls', set_current=False)",
        "py rasm.set_layer('walls')",
        "py rasm.add_line((0,0),(10,0))",
        "py rasm.add_layer('doors')",
        "py rasm.add_circle((0,0), 1)",
    ], "rasm.doc.layers()", "rasm.doc.active_layer()",
       "rasm.doc.get(0)", "rasm.doc.get(1)")
    by_name = {l["name"]: l for l in layers}
    assert active == by_name["doors"]["id"]
    assert d0["layer"] == "walls"
    assert d1["layer"] == "doors"
