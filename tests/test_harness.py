"""Smoke tests: binary runs, empty scene, help/list, unknown commands."""

import pytest

from tests.cadcli import check_obj


def test_empty_doc_scene(cadcli):
    res = cadcli.run([]).expect_ok()
    assert res.dobjects == []
    assert res.intersections == []
    assert res.intersection_count == 0
    assert "=== dobjects (0) ===" in res.raw_stdout
    assert "=== intersections ===" in res.raw_stdout
    assert res.raw_stderr == ""


def test_blank_lines_and_comments_are_ignored(cadcli):
    res = cadcli.run(["", "   ", "# a comment", "line 0,0 10,0", "# again",
                      ""]).expect_ok()
    assert len(res.dobjects) == 1


def test_help_lists_all_commands(cadcli):
    res = cadcli.run(["help"]).expect_ok()
    joined = "\n".join(res.replies)
    for expected in ("commands:", "line  x1,y1 x2,y2",
                     "circle cx,cy r", "arc   cx,cy r start_deg end_deg",
                     "arc3p p1 p2 p3", "arcse cx,cy start end",
                     "arccr start end r [major|minor]",
                     "arccl start end length [left|right]",
                     "del N / clear / help", "python:",
                     "py <expr>", "pyfile <path>", "run <name> [args…]"):
        assert expected in joined, f"help missing {expected!r}:\n{joined}"


def test_help_alias_question_mark(cadcli):
    res = cadcli.run(["?"]).expect_ok()
    assert any("commands:" in r for r in res.replies)


def test_list_on_empty_doc(cadcli):
    res = cadcli.run(["list"]).expect_ok()
    assert any(r == "list — all dobjects:" for r in res.replies)


def test_list_after_adds_matches_scene_dump(cadcli):
    res = cadcli.run(["line 0,0 10,0", "circle 5,5 2", "list"]).expect_ok()
    listed = [r for r in res.replies if r.startswith("  #")]
    assert len(listed) == 2
    assert len(res.dobjects) == 2
    for i, d in enumerate(res.dobjects):
        assert listed[i] == f"  #{i} {d.raw}"


def test_unknown_command_is_parse_error(cadcli):
    res = cadcli.run(["foobar 1 2 3"]).expect_ok(allow_errors=True)
    res.expect_error_containing("! parse error: unknown command 'foobar'")


def test_no_panic_or_stderr_never(cadcli):
    # A battery of nonsense must never panic the process or write stderr.
    res = cadcli.run([
        "line", "circle 0,0", "arc 1 2 3 4 5 6", "del", "clear", "help",
        "run", "py", "pyfile", "grips", "end", "list", "select", "all",
        "undo", "move", "hatch ANSI31", "unknown-command", "arc3p 0,0 1,1",
        "circle 0,0 -1", "del -5", "save x.rsm", "open y.dxf",
    ]).expect_ok(allow_errors=True)
    assert res.raw_stderr == ""
