"""rasm.save / rasm.open: real .rsm/.dxf output verified by the Python
binary reader, determinism, and loud error paths.

All scenarios run their draws + saves in ONE process."""

import re

import pytest

from tests import rsm_reader
from tests.cadcli import jrun

DXF_ENTITY_NAMES = {"LINE", "CIRCLE", "ARC", "ELLIPSE", "POINT", "LWPOLYLINE",
                    "POLYLINE", "TEXT", "HATCH", "SPLINE", "WALL", "DIMENSION"}


def _save_script(path, setup=()):
    return list(setup) + [f"py rasm.save({str(path)!r})"]


def _saved_bytes(res):
    vals = [r[2:] for r in res.replies if r.startswith("= ")]
    assert vals, f"no save reply: {res.errors} {res.tracebacks}"
    m = re.search(r"\((\d+) bytes\)", vals[-1])
    assert m, vals[-1]
    return int(m.group(1))


def _dxf_entity_count(text):
    lines = text.splitlines()
    count = 0
    for i in range(len(lines) - 1):
        if lines[i] == "0" and lines[i + 1] in DXF_ENTITY_NAMES:
            count += 1
    return count


def test_save_rsm_writes_valid_binary(cadcli, tmp_path):
    path = tmp_path / "t.rsm"
    res = cadcli.run(_save_script(path,
                                  ["line 0,0 10,0", "circle 5,5 2"])).expect_ok()
    assert _saved_bytes(res) > 0
    data = path.read_bytes()
    info = rsm_reader.read_rsm(data)
    # The plan expected "version u16 = 1"; the writer emits the current
    # VERSION constant (cad_io/src/rsm.rs) — asserted as-is. The SIMLUX fork
    # renumbered its merged format to VERSION 200 (2026-08-29 merge decision).
    assert info["version"] == 200
    assert info["dobject_count"] == 2
    # geom tags: 0 = Line, 1 = Circle
    assert info["first_geom_tag"] == 0


def test_save_rsm_empty_doc(cadcli, tmp_path):
    path = tmp_path / "empty.rsm"
    cadcli.run(_save_script(path)).expect_ok()
    info = rsm_reader.read_rsm(path.read_bytes())
    assert info["dobject_count"] == 0


def test_save_rsm_deterministic_bytes(cadcli, tmp_path):
    a = tmp_path / "a.rsm"
    b = tmp_path / "b.rsm"
    cadcli.run(_save_script(a, [
        "line 0,0 10,0", "circle 5,5 2", "arc 0,0 3 0 90",
        "ellipse 0,0 5,0 2", "point 1,1",
    ]) + [f"py rasm.save({str(b)!r})"]).expect_ok()
    assert a.read_bytes() == b.read_bytes()
    info = rsm_reader.read_rsm(a.read_bytes())
    assert info["dobject_count"] == 5


def test_save_rsm_count_matches_scene(cadcli, tmp_path):
    path = tmp_path / "t.rsm"
    cadcli.run(_save_script(path, [
        "line 0,0 10,0", "circle 5,5 2", "polyline 0,0 10,0 10,10 close",
        "del 1",
    ])).expect_ok()
    info = rsm_reader.read_rsm(path.read_bytes())
    assert info["dobject_count"] == 2


def test_save_dxf_ascii_and_entities(cadcli, tmp_path):
    path = tmp_path / "t.dxf"
    cadcli.run(_save_script(path, [
        "line 0,0 10,0", "circle 5,5 2",
        "py rasm.add_text('Hello', (1,1), 2.5)",
    ])).expect_ok()
    text = path.read_text(encoding="utf-8", errors="replace")
    assert "ENTITIES" in text
    assert _dxf_entity_count(text) == 3
    assert "SECTION" in text


def test_save_dxf_empty_doc(cadcli, tmp_path):
    path = tmp_path / "e.dxf"
    cadcli.run(_save_script(path)).expect_ok()
    assert _dxf_entity_count(path.read_text(errors="replace")) == 0


def test_save_bad_extension(cadcli, tmp_path):
    path = tmp_path / "t.bad"
    res = cadcli.run(_save_script(path)).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: save", "unknown extension (expected .dxf or .rsm)")


def test_save_unwritable_path(cadcli, tmp_path):
    path = tmp_path / "no_such_dir" / "t.rsm"
    res = cadcli.run(_save_script(path)).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing("RuntimeError: save")


def test_open_headless_error(cadcli, tmp_path):
    path = tmp_path / "t.rsm"
    res = cadcli.run(_save_script(path, ["line 0,0 10,0"]) + [
        f"py rasm.open({str(path)!r})",
    ]).expect_ok(allow_tracebacks=True)
    res.expect_traceback_containing(
        "RuntimeError: open is not available in the headless CLI")
    # document untouched (same process read-back)
    _, (count,) = jrun(cadcli, _save_script(path, ["line 0,0 10,0"]) + [
        f"py rasm.open({str(path)!r})",
    ], "rasm.doc.count()", allow_tracebacks=True)
    assert count == 1
