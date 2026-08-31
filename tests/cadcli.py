"""Black-box client for the headless `cad_cli` binary.

One :class:`CadCli` instance spawns one fresh subprocess per :meth:`CadCli.run`
call, so every scenario runs against a brand-new `Document` (full isolation).

Protocol (see cad_cli/src/main.rs): commands are read from stdin line by line
(blank lines and ``#`` comments skipped). Replies go to stdout. At EOF the
process prints a blank line, ``=== dobjects (N) ===``, one ``  #N <describe>``
line per dobject, a blank line, ``=== intersections ===``, the intersection
points and ``total: N``.

All `describe()` format parsing lives here — single point of change if the
Rust formatting ever changes.
"""

from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

TOL = 1e-4  # scene dump rounds coordinates to 4 decimals

# Lines that start a reply (anything else in the reply section is script
# print output, which is also collected as a reply).
_REPLY_START = (
    "+ ", "- ", "= ", "py> ", "pyfile> ", "run> ", "! ",
    "( ", "commands:", "list \u2014", "run: ", "python: ",
)


def _is_reply_line(line: str) -> bool:
    return line.startswith(_REPLY_START)


# ---------------------------------------------------------------------------
# describe() parsers — one regex per Geom kind (cad_cli/src/main.rs describe())
# ---------------------------------------------------------------------------

_NUM = r"-?[\d.]+"
# name is a PT name -> consumes 2 groups (x, y)
_PT = {"a", "b", "center", "at", "start", "end", "base", "dir", "insert"}
# name is an integer scalar (or string that stays a string)
_INT = {"style", "verts", "closed", "boundary_loops", "degree",
        "control_points", "rows", "cols", "children", "block", "pts"}

_KIND_PARSERS = [
    ("line", re.compile(
        rf"^line \(({_NUM}),({_NUM})\) -> \(({_NUM}),({_NUM})\)$"),
     ["a", "b"]),
    ("circle", re.compile(
        rf"^circle c=\(({_NUM}),({_NUM})\) r=({_NUM})$"),
     ["center", "radius"]),
    ("arc", re.compile(
        rf"^arc c=\(({_NUM}),({_NUM})\) r=({_NUM}) start=({_NUM})° "
        rf"sweep=({_NUM})°$"),
     ["center", "radius", "start_deg", "sweep_deg"]),
    ("ellipse", re.compile(
        rf"^ellipse c=\(({_NUM}),({_NUM})\) a=({_NUM}) ratio=({_NUM}) "
        rf"rot=({_NUM})°$"),
     ["center", "semi_major", "ratio", "rot_deg"]),
    ("ellipsearc", re.compile(
        rf"^ellipsearc c=\(({_NUM}),({_NUM})\) a=({_NUM}) ratio=({_NUM}) "
        rf"start=({_NUM})° sweep=({_NUM})°$"),
     ["center", "a", "ratio", "start_deg", "sweep_deg"]),
    ("point", re.compile(
        rf"^point \(({_NUM}),({_NUM})\) style=(\d+) size=({_NUM})$"),
     ["at", "style", "size"]),
    ("polyline", re.compile(
        rf"^polyline (\d+) verts( \(closed\))? length=({_NUM})$"),
     ["verts", "closed", "length"]),
    ("hatch", re.compile(r"^hatch \((\d+) boundary loops, (.*)\)$"),
     ["boundary_loops", "pattern"]),
    ("text", re.compile(
        rf'^text "(.*)" @ \(({_NUM}),({_NUM})\) h=({_NUM}) ang=({_NUM})°$'),
     ["text", "at", "height", "angle_deg"]),
    ("spline", re.compile(r"^spline \(degree (\d+), (\d+) control points\)$"),
     ["degree", "control_points"]),
    ("wall", re.compile(
        rf"^wall \(({_NUM}),({_NUM})\) -> \(({_NUM}),({_NUM})\) thk=({_NUM})\)$"),
     ["start", "end", "thickness"]),
    ("dim", re.compile(rf"^dim (\S+) value=({_NUM}) style=(\S+)$"),
     ["kind", "value", "style"]),
    ("xline", re.compile(
        rf"^xline base=\(({_NUM}),({_NUM})\) dir=\(({_NUM}),({_NUM})\)$"),
     ["base", "dir"]),
    ("ray", re.compile(
        rf"^ray base=\(({_NUM}),({_NUM})\) dir=\(({_NUM}),({_NUM})\)$"),
     ["base", "dir"]),
    ("donut", re.compile(
        rf"^donut center=\(({_NUM}),({_NUM})\) r=({_NUM})->({_NUM})$"),
     ["center", "inner_radius", "outer_radius"]),
    ("wipeout", re.compile(r"^wipeout (\d+) vertices$"), ["verts"]),
    ("region", re.compile(r"^region (\d+) vertices$"), ["verts"]),
    ("table", re.compile(
        rf"^table (\d+)×(\d+) at \(({_NUM}),({_NUM})\)$"),
     ["rows", "cols", "insert"]),
    ("xref", re.compile(
        rf"^xref '(.+)' -> (.+) at \(({_NUM}),({_NUM})\) \((\d+) children\)$"),
     ["name", "path", "insert", "children"]),
    ("blockref", re.compile(
        rf"^blockref #(\d+) at \(({_NUM}),({_NUM})\) scale=({_NUM}) "
        rf"rot=({_NUM})\)$"),
     ["block", "insert", "scale", "rot_deg"]),
    ("viewport", re.compile(
        rf"^viewport \(({_NUM}),({_NUM})\) ({_NUM})x({_NUM})$"),
     ["center", "width", "height"]),
    ("leader", re.compile(r'^leader \((\d+) pts, text "(.*)"\)$'),
     ["pts", "text"]),
    ("attdef", re.compile(
        rf'^attdef "(.*)" at \(({_NUM}),({_NUM})\) h=({_NUM})$'),
     ["tag", "at", "height"]),
    ("centermark", re.compile(
        rf"^centermark at \(({_NUM}),({_NUM})\) size=({_NUM}) rot=({_NUM})°$"),
     ["center", "size", "rot_deg"]),
]


def _conv(s: str):
    if s is None:
        return None
    try:
        return float(s)
    except ValueError:
        return s


def _consume(names: list, groups: tuple) -> dict:
    out: dict = {}
    gi = 0
    for name in names:
        if name == "closed":
            out[name] = groups[gi] is not None
            gi += 1
        elif name in _PT:
            out[name] = (float(groups[gi]), float(groups[gi + 1]))
            gi += 2
        elif name in _INT:
            g = groups[gi]
            try:
                out[name] = int(g)
            except (ValueError, TypeError):
                out[name] = _conv(g)
            gi += 1
        else:
            out[name] = _conv(groups[gi])
            gi += 1
    return out


def parse_describe(line: str) -> "DObject":
    for kind, rx, names in _KIND_PARSERS:
        m = rx.match(line)
        if m:
            return DObject(kind=kind, fields=_consume(names, m.groups()),
                           raw=line)
    return DObject(kind="unknown", fields={"raw": line}, raw=line)


@dataclass
class DObject:
    kind: str
    fields: dict
    raw: str

    def __getitem__(self, k):
        return self.fields[k]

    def get(self, k, default=None):
        return self.fields.get(k, default)

    def approx_eq(self, other: "DObject", tol: float = TOL) -> bool:
        if self.kind != other.kind:
            return False
        if set(self.fields) != set(other.fields):
            return False
        for k, a in self.fields.items():
            b = other.fields[k]
            if isinstance(a, (int, float)) and isinstance(b, (int, float)):
                if abs(float(a) - float(b)) > tol:
                    return False
            elif a != b:
                return False
        return True

    def __repr__(self):
        return f"DObject({self.kind}, {self.fields})"


@dataclass
class CliResult:
    script: list[str]
    replies: list[str]
    errors: list[str]
    tracebacks: list[str]
    dobjects: list[DObject]
    intersections: list[tuple[float, float, int, int]]
    intersection_count: int
    exit_code: int
    raw_stdout: str
    raw_stderr: str = ""
    client: "CadCli | None" = None

    @property
    def panicked(self) -> bool:
        return ("panicked at" in self.raw_stdout
                or "panicked at" in self.raw_stderr)

    def expect_ok(self, allow_errors: bool = False,
                  allow_tracebacks: bool = False) -> "CliResult":
        """Assert the two invariants every scenario must hold:
        exit 0 and no panic output. Errors/tracebacks optional (some
        scenarios legitimately exercise them)."""
        assert self.exit_code == 0, (
            f"exit code {self.exit_code}\n--- stdout ---\n{self.raw_stdout}"
            f"\n--- stderr ---\n{self.raw_stderr}")
        assert not self.panicked, (
            f"panic output detected\n--- stdout ---\n{self.raw_stdout}"
            f"\n--- stderr ---\n{self.raw_stderr}")
        if not allow_errors and self.errors:
            raise AssertionError(
                f"unexpected error lines: {self.errors}\nscript: {self.script}")
        if not allow_tracebacks and self.tracebacks:
            raise AssertionError(
                f"unexpected tracebacks:\n{chr(10).join(self.tracebacks)}\n"
                f"script: {self.script}")
        return self

    def expect_error_containing(self, *needles: str) -> "CliResult":
        for n in needles:
            assert any(n in e for e in self.errors), (
                f"no error line containing {n!r}; errors={self.errors}")
        return self

    def expect_traceback_containing(self, *needles: str) -> "CliResult":
        for n in needles:
            assert any(n in t for t in self.tracebacks), (
                f"no traceback containing {n!r}; tracebacks={self.tracebacks}")
        return self


def check_obj(dobj: DObject, kind: str, tol: float = TOL, **fields):
    """Assert a scene-dump dobject matches kind + fields (tolerance-aware)."""
    assert dobj.kind == kind, f"kind {dobj.kind!r} != {kind!r} ({dobj.raw})"
    for k, want in fields.items():
        got = dobj.get(k)
        assert got is not None, f"missing field {k} in {dobj.raw}"
        if isinstance(want, (int, float)) and isinstance(got, (int, float)):
            assert abs(float(got) - float(want)) <= tol, (
                f"field {k}: {got} != {want} (±{tol}) in {dobj.raw}")
        elif isinstance(want, (tuple, list)) and isinstance(got, (tuple, list)):
            assert len(want) == len(got), (
                f"field {k}: {got} != {want} in {dobj.raw}")
            for gv, wv in zip(got, want):
                if isinstance(wv, (int, float)) and isinstance(gv, (int, float)):
                    assert abs(float(gv) - float(wv)) <= tol, (
                        f"field {k}: {got} != {want} (±{tol}) in {dobj.raw}")
                else:
                    assert gv == wv, (
                        f"field {k}: {got} != {want} in {dobj.raw}")
        else:
            assert got == want, f"field {k}: {got!r} != {want!r} in {dobj.raw}"


def find_dobject(res: CliResult, kind: str, index: int = 0) -> DObject:
    matches = [d for d in res.dobjects if d.kind == kind]
    assert matches, f"no {kind} dobject in scene: {res.dobjects}"
    return matches[index]


_INT_RX = re.compile(r"^\s*\(\s*(" + _NUM + r"),\s*(" + _NUM + r")\)\s+"
                     r"\[dobjects #(\d+) ∩ #(\d+)\]$")


class CadCli:
    """One scenario runner. `workdir` is the process cwd (repo root, so
    `run <name>` finds scripts/); `tmpdir` is where temp .py / save files
    are written."""

    def __init__(self, binary: str, workdir: str,
                 tmpdir: str | None = None):
        self.binary = binary
        self.workdir = workdir
        self.tmpdir = tmpdir or workdir
        self._file_counter = 0

    # -- low level ----------------------------------------------------------

    def run(self, lines: list[str], timeout: float = 120.0) -> CliResult:
        text = "\n".join(lines) + "\n"
        p = subprocess.run([self.binary], input=text, capture_output=True,
                           text=True, cwd=self.workdir, timeout=timeout)
        result = _parse_output(lines, p.stdout, p.stderr, p.returncode)
        result.client = self
        return result

    # -- sugar ---------------------------------------------------------------

    def expr(self, code: str) -> str | None:
        """Run `py <code>`; return the `= value` string (None when the
        expression evaluates to None and prints nothing). Raises on error."""
        res = self.run([f"py {code}"])
        for line in res.replies:
            if line.startswith("= "):
                return line[2:]
        if res.errors or res.tracebacks:
            raise AssertionError(
                f"py failed: {res.errors} {res.tracebacks}\n{res.raw_stdout}")
        return None

    def py_json(self, code: str):
        """Run `py import json; print(json.dumps(<code>))` and parse the
        printed line. Tuples become JSON arrays (python-side round trip).
        Returns None when the expression is None (nothing printed)."""
        res = self.run([f"py import json; print(json.dumps({code}))"])
        res.expect_ok()
        for line in reversed(res.replies):
            if line.startswith(("py> ", "= ")):
                continue
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                continue
        return None

    def py_file(self, code: str, name: str | None = None) -> CliResult:
        """Write code to a temp .py in tmpdir and run `pyfile <path>`."""
        if name is None:
            self._file_counter += 1
            name = f"t{self._file_counter}.py"
        path = Path(self.tmpdir) / name
        path.write_text(code)
        return self.run([f"pyfile {path}"])

    def json_get(self, index: int):
        """`rasm.doc.get(i)` as JSON — exact floats (no scene rounding).

        NOTE: runs in a FRESH process (empty document). Only useful for
        probes; prefer :func:`jrun` for stateful scenarios.
        """
        return self.py_json(f"rasm.doc.get({index})")


def jrun(cadcli: CadCli, lines, *exprs, allow_errors: bool = False,
         allow_tracebacks: bool = False):
    """Run `lines` and, in the SAME process, print json.dumps of each
    expression (marked `J<i>>>` lines). Returns (CliResult, [values]).

    This is the correct way to read back state after a scenario — one
    subprocess, one document."""
    extra = [f"py import json; print('J{i}>>>' + json.dumps({e}))"
             for i, e in enumerate(exprs)]
    res = cadcli.run(list(lines) + extra).expect_ok(
        allow_errors=allow_errors, allow_tracebacks=allow_tracebacks)
    vals = []
    for i in range(len(exprs)):
        mark = f"J{i}>>>"
        found = None
        for line in res.replies:
            if line.startswith(mark):
                found = json.loads(line[len(mark):])
                break
        vals.append(found)
    return res, vals


# ---------------------------------------------------------------------------
# output parsing
# ---------------------------------------------------------------------------

def _parse_output(lines: list[str], stdout: str, stderr: str,
                  exit_code: int) -> CliResult:
    dobjects: list[DObject] = []
    intersections: list[tuple[float, float, int, int]] = []

    m = re.search(r"^=== dobjects \((\d+)\) ===$", stdout, re.M)
    if not m:
        raise AssertionError(f"no scene dump in output:\n{stdout}")
    body = stdout[: m.start()].rstrip("\n")
    dump = stdout[m.end():]
    n_declared = int(m.group(1))

    total_m = re.search(r"^total: (\d+)$", dump, re.M)
    assert total_m, f"no `total:` line in scene dump:\n{dump}"
    total = int(total_m.group(1))
    inter_section = dump[: total_m.start()]

    im = re.search(r"^=== intersections ===$", inter_section, re.M)
    assert im, f"no `=== intersections ===` in scene dump:\n{dump}"
    inter_body = inter_section[im.end():].strip("\n")

    for dline in dump[: im.start()].splitlines():
        dm = re.match(r"^  #\d+ (.*)$", dline)
        if not dm:
            continue
        dobjects.append(parse_describe(dm.group(1)))
    assert len(dobjects) == n_declared, (
        f"declared {n_declared} dobjects but parsed {len(dobjects)}")

    for iline in inter_body.splitlines():
        im2 = _INT_RX.match(iline)
        if not im2:
            continue
        intersections.append((float(im2.group(1)), float(im2.group(2)),
                              int(im2.group(3)), int(im2.group(4))))
    assert len(intersections) == total, (
        f"total {total} but parsed {len(intersections)} intersection lines")

    replies, errors, tracebacks = _parse_reply_section(body)
    return CliResult(script=lines, replies=replies, errors=errors,
                     tracebacks=tracebacks, dobjects=dobjects,
                     intersections=intersections,
                     intersection_count=total, exit_code=exit_code,
                     raw_stdout=stdout, raw_stderr=stderr)


def _parse_reply_section(body: str):
    replies: list[str] = []
    errors: list[str] = []
    tracebacks: list[str] = []
    body_lines = body.splitlines()
    i = 0
    while i < len(body_lines):
        line = body_lines[i]
        if line.startswith("Traceback (most recent call last):"):
            block = [line]
            i += 1
            while i < len(body_lines) \
                    and body_lines[i].startswith("  File "):
                block.append(body_lines[i])
                i += 1
            if i < len(body_lines):
                block.append(body_lines[i])  # the exception line
                i += 1
            tracebacks.append("\n".join(block))
            continue
        if line.startswith("! "):
            errors.append(line)
        else:
            replies.append(line)
        i += 1
    return replies, errors, tracebacks
