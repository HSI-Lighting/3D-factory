# RUST-AutoRASM Python Scripting — API Reference for AI Agents

This document is the complete, authoritative reference for writing CAD
scripts for RUST-AutoRASM. It is written to be consumed by an AI agent
(code model) that generates scripts on a user's behalf. Follow it exactly:
every function, type, convention, and limitation below is verified against
the implementation.

---

## 1. System overview

Scripts are Python files (CPython 3.11+, full standard library available)
run inside the CAD app by an embedded interpreter on a dedicated worker
thread. The UI never blocks. The script talks to the document ONLY through
the `rasm` module — there is no other supported way to touch the drawing
(no direct access to the internal document object).

Key properties every generated script must respect:

- **One run = one undo unit.** Everything a single run draws/deletes is one
  `Ctrl+Z` step. `rasm.undo_group()` inside a run splits it: each boundary
  becomes its own undo unit (pre-run + one per boundary). Never try to
  batch multiple "jobs" into one run without boundaries.
- **Esc cancels** a running script (KeyboardInterrupt). Long loops should
  not be used to build drawings — batch through the API below.
- **The script may execute MORE THAN ONCE per invocation**: while the
  parameter dialog is open, the host runs *ghost preview* passes with every
  document op no-op'd (see §7). A run is also preceded by a *metadata pass*
  that executes the whole file with ops no-op'd. Consequence: keep module
  top-level code cheap and side-effect-free (imports + helper definitions +
  `rasm.main(...)` only). All drawing must live inside the main function.
- **Scene units.** All geometry coordinates and radii are in *scene units*
  (the document's internal unit). `length` inputs convert from the user's
  display unit automatically — see §5. Angles are in degrees everywhere in
  the API; trig done with `math` uses radians (convert yourself).
- Scripts are **trusted code** (no sandbox). They can read files the user
  can read. Use that power responsibly; never touch files outside the
  script's own purpose.

---

## 2. Invoking scripts (what the user types)

| Form | Meaning |
|---|---|
| `py <expr>` | Run one Python expression/statement (REPL; an expression echoes its value). |
| `py` | Toggle the docked Python console panel. |
| `pyfile <path.py>` | Run a `.py` file from any path. |
| `run <name>` | Run `scripts/<name>.py`; opens the parameter dialog when the script declares inputs. |
| `run <name> k=v k2=v2 …` | Run immediately with NAMED inputs (skips the dialog). |
| `run <name> v1 v2 …` | Legacy positional inputs, mapped onto the declared parameter order. |
| `run` | List the available scripts in `scripts/`. |
| `pyhelp` / `rasmhelp` | Show this reference inside the app. |

Scripts live in the `scripts/` folder (also found next to the executable).
The same script can be run from: the command bar, Tools → Scripts ▸ menu,
the console's example row, the in-app script editor (save + run), or
headlessly via `cad_cli` (`run <name> k=v …`).

---

## 3. Declaring inputs — `rasm.main(fn)`

The ONLY supported way for a script to declare typed, named inputs:

```python
def run(outer_d: 'length' = 120.0, bolts=6, label: 'str' = 'P1',
        pos: 'point' = (0.0, 0.0), holes_color: 'color' = 5):
    """Parametric pipe flange.
    outer_d: outer diameter (10..500)
    bolts: number of bolts (3..24)
    label: part label
    pos: center position
    holes_color: hole color
    """
    ... draw everything here ...
    return  # optional

rasm.main(run)
```

Rules:

1. The function's **signature IS the declaration**. Types come from string
   annotations when present, else from the defaults' Python types.
2. **Types**: `float`, `int`, `bool`, `str`, `length`, `point`, `entity`,
   `color`, `choice`, `linetype`, `layer`, `block`, `hatch_pattern`,
   `float_list`, `int_list`, `str_list`, `point_list`.
   - `length`: annotate with `'length'`. The user enters display units
     (suffixes allowed: `25`, `25cm`, `6'`); the function receives SCENE
     units. See §5.
   - `point`: tuple default `(x, y)` or `'point'` annotation. The function
     receives a `(x, y)` tuple of scene coordinates. In the dialog the user
     can pick it by clicking the canvas.
   - `entity`: `'entity'` annotation (default -1). The user clicks an
     EXISTING shape on the canvas; the function receives its index
     (or -1 when unpicked).
   - `color`: `'color'` annotation on an int default. The function receives
     an ACI color number (0–255). The dialog opens the ACI color wheel.
   - `choice`: `'choice'` annotation. The docstring line
     `name: help [a, b, c]` declares the options; the dialog shows a
     DROPDOWN, and `run <name> name=X` validates X against the list
     (loudly). The function receives the selected string.
   - `linetype` / `layer` / `block` / `hatch_pattern`: catalog-backed
     DROPDOWNS filled from the LIVE document (the linetype catalog, the
     existing layers, the block definitions, the hatch-pattern catalog).
     Values validate against the catalogs on every path (dialog, `run
     <name> k=v …`, positional) and canonicalize case-insensitively.
   - `float_list` / `int_list` / `str_list`: comma-separated values
     (`"1, 2.5, 3"`) → `list[float]` / `list[int]` / `list[str]`. The
     dialog shows one text field. `point_list`: `"x,y; x,y; …"` →
     `list[(x, y)]`.
3. **Docstring** lines `name: help text` provide the dialog's help text.
   `name: help (min..max)` adds a numeric range (dialog clamps and shows it).
4. **Defaults** are used when the user provides nothing; they are scene
   units for `length`/`point` (displayed converted).
5. `rasm.main(fn)` converts inputs and calls `fn(**values)`. Named inputs
   (`k=v`) fill by name; missing ones use defaults; positional inputs map
   onto the declared order. A bad value aborts the run loudly
   (`! name: 'x' is not a valid float`).
6. Inside the function the script reads its other inputs as needed:
   `rasm.args` (positional extras as `list[str]`) and `rasm.params`
   (raw named values as `dict[str, str]`; lengths already scene-converted).
   Prefer declared parameters over these.

---

## 4. The `rasm` API — full reference

All functions raise a Python exception (with a message) on invalid input;
the app shows the traceback in the command history. Never catch-and-hide
errors — the user must see them.

### 4.1 Drawing (each returns the new entity's INDEX)

```python
rasm.add_line(a, b)                          # a, b = (x, y) tuples
rasm.add_circle(center, radius)
rasm.add_arc(center, radius, start_deg, sweep_deg)
rasm.add_ellipse(center, major, ratio)       # major = (dx, dy) vector; ratio < 1
rasm.add_polyline(points, closed=False)      # points = list[(x, y)], len >= 2
rasm.add_point(at)
rasm.add_text(text, at, height=2.5, angle_deg=0.0)
```

- Coordinates/radii are scene units; angles degrees, counter-clockwise
  positive (matching the app).
- New entities go to the **currently active layer** with the app's current
  style — exactly like the user drawing them.
- Returns: the model-space index of the new entity (or the index within the
  active layout when in layout mode).

### 4.2 Modifying existing entities (in place, undoable)

```python
rasm.move(indices, dx, dy)                 # translate; returns count moved
rasm.copy(indices, dx, dy)                 # translate COPIES; returns the NEW indices
rasm.rotate(indices, center, angle_deg)
rasm.scale(indices, center, factor)        # factor must be > 0
rasm.mirror(indices, a, b)                 # axis line a→b
rasm.set_color(indices, color=None)        # int 0..=255, 'bylayer', 'byblock', None
rasm.set_layer_of(indices, name)           # move entities onto another layer
rasm.set_linetype(indices, name=None)      # None/'' = ByLayer
rasm.set_lineweight(indices, mm=None)      # negative = ByLayer
rasm.set_visible(indices, visible)
```

- All take a LIST of model-space indices; all are one undo step per call.
- Indices are validated loudly: if none of them exist the call raises.
- Hatch boundaries are transformed with a selected hatch (like the app's
  move/rotate/scale/mirror).

### 4.3 Shape-specific properties — `rasm.set_geom(i, entity_dict)`

Replace ONE entity's GEOMETRY while keeping its style/layer: take a
snapshot (`e = rasm.doc.get(i)`), edit the fields you need, write it back:

```python
e = rasm.doc.get(3)
e["radius"] = 25.0          # or e["start"]/e["end"]/e["text"]/…
rasm.set_geom(3, e)
```

Supported `type`s: `line`, `circle`, `arc`, `ellipse`, `polyline`, `point`,
`text` (fields exactly as the entity dicts in §4.7). Other types fail
loudly. The entity's index, style, layer and handle are unchanged.

### 4.4 Hatching

```python
rasm.hatch_patterns()                  # -> list[str]  catalog ("SOLID", "ANSI31", "BRICK", …)
rasm.hatch_at(point, pattern)          # trace the smallest closed region around the
                                       # point (islands included) and hatch it
                                       # -> list[int] boundary indices (EMPTY = no region)
rasm.add_hatch(boundary_indices, pattern)   # hatch explicit boundaries
                                       # -> int  new hatch index
```

- `pattern` = `"SOLID"` or any catalog name (case-insensitive, validated
  loudly).
- `add_hatch` accepts closed polylines / circles / ellipses / closed
  splines; other boundary kinds are rejected loudly (an error lists the
  requirement).
- `hatch_at` is the app's native region tracer — the robust way to
  "find a closed area". In the headless CLI it raises (no tracer); the
  sample `scripts/auto_hatch.py` degrades gracefully.
- Hatch boundaries stay LIVE: editing a boundary later reshapes the fill.
- See `scripts/auto_hatch.py`: searches the whole scene, hatches every
  closed area (native regions + loops of touching segments, which it
  joins into closed polylines), pattern chosen via a `choice` dropdown.

### 4.5 Deleting / selection

```python
rasm.delete(indices)            # list[int]; returns count removed
rasm.selection()                # -> list[int] current selection
rasm.set_selection(indices)     # replaces selection; out-of-range dropped
                                # returns the validated list
```

- `delete` removes by model-space index, highest-first internally, so a
  mixed index list is safe. The app prunes the selection accordingly.

### 4.6 Layers

```python
rasm.add_layer(name, set_current=True)     # -> new layer id (int)
rasm.set_layer(name)                       # make the named layer active
rasm.layer_set(name, visible=None, locked=None, frozen=None,
               plottable=None, color=None) # color = ACI int; None = unchanged
```

- Layer names must be unique, non-empty. Adding an existing name fails.
- The ACTIVE layer cannot be turned off or frozen (fails loudly).
- New layers are white (ACI 7), continuous, thin, unlocked, plottable.

### 4.7 Blocks

```python
rasm.create_block(name, base)              # consumes the CURRENT selection
rasm.insert_block(name, at, rotation_deg=0.0)
```

- `create_block` turns the current selection into a block definition and
  replaces the selection with one instance at `base` (mirrors the `block`
  command). Requires a non-empty selection (`rasm.set_selection` first);
  names must be new.
- `insert_block` inserts plain blocks only. PARAMETRIC blocks fail with a
  message (insert those interactively).

### 4.8 Commands / sysvars / view / files

```python
rasm.command("line 0,0 10,0")   # run any interactive command line
                                # -> list[str] transcript lines
rasm.sysvar(name)               # -> str | None
rasm.setvar(name, value)        # persists like the SETVAR command
rasm.view()                     # -> {"center": (x, y), "scale": px_per_unit}
rasm.set_view(center, scale=None)  # scale None = pan only, must be > 0
rasm.zoom_extents()             # like `zoom e`
rasm.save(path)                 # .rsm or .dxf by extension -> transcript
rasm.open(path)                 # replaces the document -> transcript
rasm.set_layout(name)           # switch paper-space layout
rasm.undo_group()               # boundary: next undo unit starts here
rasm.set_current_color(color=None)      # style for the script's NEW entities
rasm.set_current_linetype(name)
rasm.set_current_lineweight(mm)
```

- `rasm.command(...)` drives the real command engine ("line x1,y1 x2,y2",
  "circle cx,cy r", "erase"/"delete", "zoom e", "units mm", …). It returns
  the history lines the command produced — good for verifying success.
  Commands that need clicks enter their interactive flow and finish with
  the user.
- `save`/`open` return transcript lines; a failed save appears as a `!` line.

### 4.9 Read surface — `rasm.doc`

```python
rasm.doc.count()               # -> int  (model-space entity count)
rasm.doc.get(i)                # -> dict snapshot of entity i (IndexError if out of range)
rasm.doc.entities()            # -> list[dict]  EVERY entity (O(N) — explicit)
rasm.doc.layers()              # -> list[dict]  all layers
rasm.doc.active_layer()        # -> int  active layer id
rasm.doc.blocks()              # -> list[str]  block definition names
rasm.doc.units()               # -> {"name": "mm", "scene_per_unit": 1.0}
rasm.doc.bounds()              # -> {"min": (x, y), "max": (x, y)} or None
rasm.doc.layouts()             # -> list[dict]  {id, name, active} paper-space layouts
rasm.doc.linetypes()           # -> list[str]  linetype catalog names
```

Entity dict keys (all have `handle` (int), `layer` (str name), `type`,
plus the RESOLVED style: `color` ("aci N" / "bylayer" / "byblock" /
"#RRGGBB"), `linetype` (name), `lineweight` (mm), `visible` (bool)):

| type | extra keys |
|---|---|
| line | `start`, `end` — (x, y) tuples |
| circle | `center`, `radius` |
| arc | `center`, `radius`, `start_deg`, `sweep_deg` |
| ellipse | `center`, `major` (dx, dy), `ratio` |
| ellipse_arc | `center`, `major`, `ratio`, `start_param_deg`, `sweep_param_deg` |
| point | `at`, `pdmode`, `pdsize` |
| polyline | `points` list[(x,y)], `closed`, `bulges`, `widths` |
| wall | `start`, `end`, `thickness` |
| text | `text`, `at`, `height`, `angle_deg` |
| hatch | `boundary_loops`, `pattern` |
| spline | `degree`, `control_points` |
| dimension | `kind`, `value` |
| blockref | `block` (id), `at`, `scale`, `rotation_deg` |
| viewport | `center`, `width`, `height` |

Layer dict keys: `id`, `name`, `visible`, `locked`, `frozen`, `plottable`,
`color` (a display string like `"Aci(7)"` — parse the number out if needed).

Values are OWNED COPIES — safe to hold; they do not update live. To
write back edits, use `rasm.set_geom(i, dict)` (§4.3) — the geometry keys
are the ones you may change.

### 4.10 Per-run inputs

```python
rasm.args    # list[str] — positional extras (always present, [] for plain py)
rasm.params  # dict[str, str] — raw named inputs (lengths scene-converted)
sys.argv     # ["<script name>", *positional args] — only for `run <name>`
```

---

## 5. Units and lengths

- The document has a display unit (`Units.name`: "mm", "cm", "m", "in",
  "ft", …) and a calibration `scene_per_unit` (scene units per display
  unit). Geometry is always stored in scene units.
- `'length'` parameters are the ONLY unit-aware inputs:
  - dialog field: edited in display units (unit name shown as suffix);
  - command line: `outer_d=25` = 25 display units; explicit suffixes
    convert physically regardless of the document unit (`25cm` = 250 mm,
    `1in`, `6'`, `0.15m`).
  - the function receives the SCENE-unit value (float).
- The script's own computed distances are scene units — consistent with
  everything it reads back via `rasm.doc`.
- Do NOT reimplement unit conversion in scripts; declare `'length'` params
  and let the host convert.

---

## 6. Errors, validation, and output

- `print(...)` output streams to the command history and the Python
  console (live, capped log). Use it for progress and results.
- Validation failures: `raise SystemExit("! flange: bore must be < outer")`.
  The message is shown as-is (`!` prefix = error styling). Other exceptions
  show a full traceback — still fine, but prefer a clear `SystemExit`
  message for expected/user-facing failures.
- Never silently swallow errors inside a script — re-raise or convert to a
  clear `SystemExit`.
- Idempotency: scripts are often re-run (and ghost-previewed). Creating a
  layer that already exists FAILS — check first (see the flange example):
  ```python
  layers = {l["name"]: l for l in rasm.doc.layers()}
  if "my_layer" not in layers:
      rasm.add_layer("my_layer", set_current=False)
  rasm.set_layer("my_layer")
  ```

---

## 7. Execution lifecycle (what the host does around your script)

1. **Metadata pass** (`run <name>` with no inputs, or menu pick): the file
   executes once with every rasm op no-op'd; `rasm.main` records the
   declaration; the parameter dialog opens prefilled with defaults. Nothing
   is drawn.
2. **Ghost preview**: while the dialog is open, every input change restarts
   a ghost pass (throttled ~300 ms). Writes go to a throwaway shadow
   document and render as a dashed cyan overlay — NOTHING commits, no undo
   entry, no history lines. In preview mode: reads see the shadow snapshot;
   `rasm.command` only simulates direct geometry adds; block ops answer
   with an error; `save`/`open`/`set_view`/`setvar`/selection writes are
   inert no-ops. Your script must be able to run to completion with ops
   no-op'd (it will be exercised this way). Hatches are not ghosted.
3. **Real run**: ops commit to the document; at the end all snapshots
   collapse to ONE undo unit. Prints/errors stream live; Esc cancels.

Because of (1) and (2): no Python-side side effects at module top level,
and no external file writes during preview — keep scripts pure with respect
to everything except the rasm document API.

---

## 8. Authoring conventions for AI agents

1. `import` + helper functions/constants at top level; the entire drawing
   logic inside the main function; end with `rasm.main(main_fn)`.
2. Declare EVERY user-facing number as a parameter — `'length'` for
   distances, `'point'` for positions, `'color'` for colors, ints for
   counts, bools for toggles. Give each a docstring help line (with a
   sensible `(min..max)` range where meaningful).
3. Validate inputs early with `raise SystemExit("! …")` (positive
   diameters, bore < outer, ≥3 bolts, etc.) — the ghost preview will
   exercise them, and the user gets the message instantly.
4. Draw with the `add_*` API; use `rasm.command(...)` only for features
   with no API yet.
5. Be idempotent about layers/blocks (check `rasm.doc.layers()` /
   `rasm.doc.blocks()` before creating).
6. Performance: documents can hold 1M+ entities. Never loop over
   `rasm.doc.entities()` inside a loop; prefer `count()` and index math.
   Keep generated entity counts reasonable; report what you drew with
   `print`.
7. End by fitting the view: `rasm.set_view(center, scale)` (scale =
   pixels per world unit; ~500/outer_size fits a part).
8. Follow the working examples in `scripts/`: `flange.py` (full typed-param
   demonstration), `grid_circles.py` (`rasm.args` positional inputs),
   `layers_demo.py`, `block_demo.py`, `hello.py`.

---

## 9. Headless CLI (`cad_cli`)

`cad_cli` runs scripts without a GUI for testing:

    printf 'run flange outer_d=150 bore_d=70 bolts=10\nlist\n' | cargo run -q -p cad_cli

- The same `rasm` surface works (including move/copy/rotate/scale/mirror,
  style setters and `set_geom`); selection is empty; blocks, sysvars and
  `open` answer with "not available headless" errors (loud, not silent);
  `rasm.save(path)` writes a real .rsm/.dxf via `cad_io`.
- `run <name> k=v …` and positional forms work; length conversion applies.
- Use it to verify a generated script end-to-end before handing it to the
  user in the GUI.

---

## 10. Quick reference card

```python
import math

def run(radius: 'length' = 50.0, count=6, pos: 'point' = (0.0, 0.0),
        color: 'color' = 4):
    """Hole pattern.
    radius: pattern radius (5..1000)
    count: number of holes (1..64)
    pos: pattern center
    color: hole color
    """
    if count < 1: raise SystemExit("! count must be >= 1")
    for k in range(count):
        a = math.radians(k * 360.0 / count)
        rasm.add_circle((pos[0] + math.cos(a) * radius,
                         pos[1] + math.sin(a) * radius), radius / 10.0)
    print("drew", count, "holes")
    rasm.set_view(pos, 600.0 / radius)

rasm.main(run)
```
