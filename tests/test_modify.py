"""del / clear / list — index math and error surfaces."""

import pytest

from tests.cadcli import check_obj


@pytest.mark.parametrize("idx", [0, 1, 2])
def test_del_first_middle_last(cadcli, idx):
    res = cadcli.run(["line 0,0 1,1", "line 2,2 3,3", "line 4,4 5,5",
                      f"del {idx}"]).expect_ok()
    assert len(res.dobjects) == 2
    assert any(r == f"- removed #{idx}" for r in res.replies)


def test_del_out_of_range(cadcli):
    res = cadcli.run(["line 0,0 1,1", "del 5"]).expect_ok(allow_errors=True)
    assert len(res.dobjects) == 1
    assert any(r == "! no dobject #5" for r in res.errors)


def test_del_out_of_range_on_empty(cadcli):
    res = cadcli.run(["del 0"]).expect_ok(allow_errors=True)
    assert res.dobjects == []
    assert any(r == "! no dobject #0" for r in res.errors)


def test_del_huge_index(cadcli):
    res = cadcli.run(["line 0,0 1,1", "del 999999"]).expect_ok(allow_errors=True)
    assert len(res.dobjects) == 1
    assert any(r == "! no dobject #999999" for r in res.errors)


def test_del_huge_overflow_parse_error(cadcli):
    res = cadcli.run(["del 99999999999999999999999999"]).expect_ok(allow_errors=True)
    assert any("bad index" in e for e in res.errors)


def test_repeated_deletes_shift_indices(cadcli):
    res = cadcli.run(["line 0,0 1,1", "line 2,2 3,3", "line 4,4 5,5",
                      "del 0", "del 0"]).expect_ok()
    assert len(res.dobjects) == 1
    check_obj(res.dobjects[0], "line", a=(4, 4), b=(5, 5))


def test_del_after_clear(cadcli):
    res = cadcli.run(["line 0,0 1,1", "clear", "del 0"]).expect_ok(allow_errors=True)
    assert res.dobjects == []
    assert any(r == "! no dobject #0" for r in res.errors)


@pytest.mark.parametrize("cmd,needle", [
    ("del", "del N"),
    ("del abc", "bad index"),
    ("del -1", "bad index"),
    ("del 1.5", "bad index"),
    ("del 1x", "bad index"),
])
def test_del_bad_args(cadcli, cmd, needle):
    res = cadcli.run([cmd]).expect_ok(allow_errors=True)
    assert any(needle in e for e in res.errors), (cmd, res.errors)


def test_clear_mid_session_then_readd(cadcli):
    res = cadcli.run(["line 0,0 1,1", "circle 5,5 2", "clear",
                      "line 9,9 8,8"]).expect_ok()
    assert len(res.dobjects) == 1
    assert any(r == "- cleared" for r in res.replies)
    check_obj(res.dobjects[0], "line", a=(9, 9), b=(8, 8))


def test_list_after_each_stage_matches_dump(cadcli):
    res = cadcli.run([
        "line 0,0 1,1", "list",
        "circle 5,5 2", "list",
        "del 0", "list",
        "clear", "list",
    ]).expect_ok()
    assert res.dobjects == []
    headers = [i for i, r in enumerate(res.replies)
               if r == "list — all dobjects:"]
    assert len(headers) == 4
    listed_idx = [i for i, r in enumerate(res.replies) if r.startswith("  #")]
    # the last list (after clear) printed no entities: no `  #` after it
    assert not any(i > headers[-1] for i in listed_idx)
    # 3 entities were listed before the clear: 3 + 2 - 1 = ... first list 1,
    # second 2, third 1 (after del 0)
    assert len(listed_idx) == 4
