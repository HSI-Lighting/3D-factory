"""Command words, aliases, arity errors, bad tokens, interactive-only replies."""

import pytest

from tests.cadcli import check_obj

IGNORED_EDIT = "(editing op ignored — CLI has no interactive selection)"
IGNORED_SELECT = "(select ignored — CLI has no interactive selection)"
IGNORED_SUB = "(selection sub-command ignored — CLI has no selection session)"
IGNORED_SNAP = "(snap override '{K}' ignored — CLI has no interactive draw)"
IGNORED_OPEN = "(open/save ignored — CLI is a math REPL, not a doc viewer)"
IGNORED_GRIPS = "(grips toggle ignored — CLI has no selection / display)"


# -- every keyword + alias adds a dobject --------------------------------------

@pytest.mark.parametrize("cmd,kind", [
    ("line 0,0 10,0", "line"),
    ("l 0,0 10,0", "line"),
    ("circle 5,5 2", "circle"),
    ("ci 5,5 2", "circle"),
    ("ellipse 0,0 5,0 2", "ellipse"),
    ("el 0,0 5,0 2", "ellipse"),
    ("point 3,4", "point"),
    ("po 3,4", "point"),
    ("polyline 0,0 10,0 10,10", "polyline"),
    ("pl 0,0 10,0 10,10", "polyline"),
    ("pline 0,0 10,0 10,10", "polyline"),
    ("arc 0,0 5 0 90", "arc"),
    ("a 0,0 5 0 90", "arc"),
    ("arc3p 0,0 5,0 0,5", "arc"),
    ("arcse 0,0 5,0 0,5", "arc"),
    ("arccr 0,0 10,0 5", "arc"),
    ("arccl 0,0 10,0 20", "arc"),
])
def test_add_aliases(cadcli, cmd, kind):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert len(res.dobjects) == 1, res.raw_stdout
    assert res.dobjects[0].kind == kind
    assert any(r.startswith("+ #0 ") for r in res.replies)


@pytest.mark.parametrize("cmd", ["del 0", "d 0"])
def test_del_alias(cadcli, cmd):
    res = cadcli.run(["line 0,0 1,1", cmd]).expect_ok()
    assert len(res.dobjects) == 0
    assert any(r == "- removed #0" for r in res.replies)


def test_clear_alias_words(cadcli):
    res = cadcli.run(["line 0,0 1,1", "clear"]).expect_ok()
    assert any(r == "- cleared" for r in res.replies)
    assert res.dobjects == []


@pytest.mark.parametrize("cmd", ["list", "ls"])
def test_list_alias(cadcli, cmd):
    res = cadcli.run(["line 0,0 1,1", cmd]).expect_ok()
    assert any("list — all dobjects:" in r for r in res.replies)


def test_py_and_python_keywords(cadcli):
    for word in ("py", "python"):
        res = cadcli.run([f"{word} 6*7"]).expect_ok()
        assert any(r == "= 42" for r in res.replies)


def test_pyfile_and_run_and_script_words(cadcli):
    res = cadcli.run(["py", "pyfile", "run", "script"]).expect_ok(allow_errors=True)
    assert any(r.startswith("! parse error: usage: pyfile") for r in res.errors)
    assert any(r.startswith("run: scripts available:") for r in res.replies)
    assert any(r.startswith("python: `py <expr>` runs inline")
               for r in res.replies)


def test_pyhelp_and_rasmhelp(cadcli):
    for word in ("pyhelp", "rasmhelp"):
        res = cadcli.run([word]).expect_ok()
        assert any(r.startswith("# RUST-AutoRASM Python Scripting")
                   for r in res.replies)


# -- bare forms → SetTool → documented CLI reply -------------------------------

@pytest.mark.parametrize("word", [
    "line", "l", "circle", "ci", "point", "po", "polyline", "pl", "pline",
    "arc", "a", "ellipse", "el",
    "spline", "spl", "qb", "quadbezier", "quadratic",
    "rectangle", "rectang", "rec", "polygon", "pol",
    "mleader", "leader", "mld", "attdef", "atd", "attedit", "ate",
    "ellipsearc", "ellipticalarc", "ellarc", "ea",
])
def test_bare_tool_words_ignored(cadcli, word):
    res = cadcli.run([word]).expect_ok()
    assert any(r == IGNORED_EDIT for r in res.replies), res.raw_stdout
    assert res.dobjects == []


# -- wrong arity -----------------------------------------------------------------

@pytest.mark.parametrize("cmd,needle", [
    ("line 0,0", "usage: line  OR  line x1,y1 x2,y2"),
    ("line 0,0 1,1 2,2", "usage: line  OR  line x1,y1 x2,y2"),
    ("circle 0,0", "usage: circle  OR  circle cx,cy r"),
    ("circle 0,0 1 2", "usage: circle  OR  circle cx,cy r"),
    ("point 1,2 3,4", "usage: point  OR  point x,y"),
    ("polyline 0,0", "usage: polyline  OR  polyline x1,y1 x2,y2"),
    ("arc 0,0 1 0", "usage: arc  OR  arc cx,cy r start_deg end_deg"),
    ("arc 0,0 1 0 90 x", "usage: arc  OR  arc cx,cy r start_deg end_deg"),
    ("arc3p 0,0 1,1", "usage: arc3p p1 p2 p3"),
    ("arc3p 0,0 1,1 2,2 3,3", "usage: arc3p p1 p2 p3"),
    ("arcse 0,0 1,1", "usage: arcse cx,cy start end"),
    ("arcse 0,0 1,1 2,2 3,3", "usage: arcse cx,cy start end"),
    ("arccr 0,0 1,1", "usage: arccr start end r [major|minor]"),
    ("arccr 0,0 1,1 2 major minor", "usage: arccr start end r [major|minor]"),
    ("arccl 0,0 1,1", "usage: arccl start end length [left|right]"),
    ("arccl 0,0 1,1 2 left right", "usage: arccl start end length [left|right]"),
    ("ellipse 0,0 1,1", "usage: ellipse  OR  ellipse cx,cy major_end_x,major_end_y minor_len"),
    ("ellipse 0,0 1,1 2 3", "usage: ellipse  OR  ellipse cx,cy major_end_x,major_end_y minor_len"),
    ("del", "del N"),
    ("pyfile", "usage: pyfile <path.py>"),
    ("open", "usage: open <path.dxf|path.rsm>"),
    ("save", "usage: save <path.dxf|path.rsm>"),
])
def test_wrong_arity(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (
        f"needle {needle!r} not in {res.errors}")


# -- bad tokens -------------------------------------------------------------------

@pytest.mark.parametrize("cmd,needle", [
    ("line a,b 1,2", "bad x: 'a'"),
    ("line 1,2,3 4,5", "expected x,y, got '1,2,3'"),
    ("line 1, 2,3", "bad y: ''"),
    ("line ,2 3,4", "bad x: ''"),
    ("line 1;2 3,4", "expected x,y, got '1;2'"),
    ("circle 0,0 abc", "bad radius"),
    ("circle 0,0 -1", "radius must be > 0"),
    ("circle 0,0 0", "radius must be > 0"),
    ("circle abc 1", "expected x,y, got 'abc'"),
    ("arc 0,0 abc 0 90", "bad radius"),
    ("arc 0,0 1 x 90", "bad start angle"),
    ("arc 0,0 1 0 x", "bad end angle"),
    ("arc 0,0 -2 0 90", "radius must be > 0"),
    ("ellipse 0,0 1,1 abc", "bad minor length"),
    ("point x", "expected x,y, got 'x'"),
    ("del abc", "bad index"),
    ("del -3", "bad index"),
    ("offset abc", "bad distance"),
    ("offset 0", "offset distance must be non-zero"),
    ("wall abc", "bad thickness"),
    ("wall 0", "wall thickness must be positive"),
    ("fillet -1", "fillet radius must be >= 0"),
    ("lengthen x", "bad delta"),
    ("chamfer -1", "chamfer distance must be >= 0"),
    ("card sideways", "card: expected `on` or `off`, got 'sideways'"),
    ("chprop layer", "chprop: usage — chprop <layer|color|linetype> <value>"),
    ("chprop bogus x", "chprop: unknown property 'bogus'"),
    ("blockdiff a", "usage: blockdiff"),
])
def test_bad_tokens(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (
        f"needle {needle!r} not in {res.errors}: got {res.errors}")


# -- interactive-only commands ------------------------------------------------------

@pytest.mark.parametrize("cmd", [
    "copy", "c", "cp", "co", "rotate", "ro", "scale", "sc",
    "mirror", "mi", "hatch", "h", "bhatch", "hatch ANSI31 2 30",
    "delete", "erase", "e", "undo", "u", "redo", "y",
    "matchprop", "mp", "reverse", "rev", "chlayer", "cl",
    "offset", "o", "offset 2.5", "wall", "w", "wall 5",
    "text", "tx", "text hello", "text \"Hello world\"",
    "style", "txtstyle", "textstyle", "style MyStyle",
    "dbg", "recorder", "linetype", "ltype", "lt", "linetype Dashed",
    "chprop", "chpr", "props", "properties", "chprop layer Walls",
    "chprop color 1", "chprop linetype Dashed",
    "dim", "dimension", "dimcontinue", "dimcont", "dimcon",
    "dimbaseline", "dimbase", "dimangular", "dimang", "dan",
    "dimarc", "dar", "dimordinate", "dimord", "dor",
    "dimjogged", "dimjog", "djo",
    "centermark", "cenm", "centermark 5",
    "xline", "xl", "ray", "donut", "doughnut", "wipeout", "wi",
    "sketch", "sk", "blend", "mline", "ml", "region", "reg",
    "qdim", "qd", "minsert", "layiso", "li", "layfrz", "layoff",
    "layon", "laywalk", "publish", "pub", "etransmit", "et",
    "measuregeom", "meas", "quickcalc", "qc",
    "find", "findtext", "find abc", "replace", "findreplace", "replace a b",
    "id", "pip", "oops", "setbylayer", "sbl",
    "rename", "ren", "rename layer a b", "revcloud", "revc", "area", "aa",
    "overkill", "ovk", "purge", "pu", "qselect", "qs",
    "pagesetup", "ps", "table", "tb", "xref", "xr", "xref attach f",
    "ucs", "ucs world", "layerstate", "layst", "layerstate save X",
    "wblock", "wb", "boundary", "bpoly", "bp",
    "dimstyle", "ddim", "dimstyle D", "wallstyle", "wstyle", "wallstyle W",
    "wallcleanup", "wcleanup",
    "block", "b", "block foo", "insert", "i", "insert foo", "explode", "xp",
    "blockdiff", "bdiff", "blockdiff a b",
    "btr", "blocktask", "taskrec", "finish", "endrec", "done",
    "card", "card on", "card off",
    "lengthen 5", "break", "br", "breakatpoint", "brp",
    "divide", "div", "measure", "me",
    "plotstyle", "pst", "stylesmanager", "plot", "print",
    "align", "stretch", "st", "s", "trim", "tr", "extend", "ex",
    "fillet", "flt", "f", "fillet 2", "chamfer", "cha", "chamfer 1 2",
    "join", "j", "dist", "di",
])
def test_interactive_only_commands(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(r == IGNORED_EDIT for r in res.replies), res.raw_stdout
    assert res.dobjects == []


@pytest.mark.parametrize("cmd", ["move", "m"])
def test_move_has_its_own_message(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(r == "(move ignored — CLI has no interactive draw)"
               for r in res.replies), res.raw_stdout
    assert res.dobjects == []


@pytest.mark.parametrize("cmd", [
    "open x.rsm", "open x.dxf", "save x.rsm", "saveas x.dxf",
])
def test_open_save_ignored(cadcli, cmd):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(r == IGNORED_OPEN for r in res.replies)


def test_snap_override_keywords(cadcli):
    for word, key in [("end", "END"), ("endpoint", "END"), ("mid", "MID"),
                      ("cen", "CEN"), ("center", "CEN"), ("qua", "QUA"),
                      ("int", "INT"), ("per", "PER"), ("tan", "TAN"),
                      ("nea", "NEA"), ("near", "NEA")]:
        res = cadcli.run([word]).expect_ok()
        assert any(r == IGNORED_SNAP.format(K=key) for r in res.replies), (
            f"{word}: {res.replies}")


def test_grips_ignored(cadcli):
    for word in ("grips", "grip"):
        res = cadcli.run([word]).expect_ok()
        assert any(r == IGNORED_GRIPS for r in res.replies)


def test_select_words_ignored(cadcli):
    res = cadcli.run(["select", "sel"]).expect_ok()
    assert any(r == IGNORED_SELECT for r in res.replies)
    for word in ("all", "prev", "previous", "before", "none", "deselect",
                 "rem", "remove", "addmode", "amode", "window", "win",
                 "crossing", "cross", "wp", "wpolygon", "cpol",
                 "cpolygon", "last"):
        res = cadcli.run([word]).expect_ok()
        assert any(r == IGNORED_SUB for r in res.replies), (
            f"{word}: {res.replies}")
    # fence maps to the generic editing-op message (observed in main.rs)
    res = cadcli.run(["fence"]).expect_ok()
    assert any(r == IGNORED_EDIT for r in res.replies)


def test_case_insensitivity(cadcli):
    res = cadcli.run(["LINE 0,0 10,0", "Circle 1,1 2", "ARC3P 0,0 5,0 0,5",
                      "DEL 2"]).expect_ok()
    assert len(res.dobjects) == 2
    assert res.dobjects[0].kind == "line"
    assert res.dobjects[1].kind == "circle"


def test_whitespace_tolerance(cadcli):
    res = cadcli.run(["   line   0,0   10,0   "]).expect_ok()
    assert len(res.dobjects) == 1
