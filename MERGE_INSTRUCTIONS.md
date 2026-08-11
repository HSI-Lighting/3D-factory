# Merging the 3D Factory / SIMLUX work

**Branch:** `fresh-main` · **Remote:** `yaseen` (`github.com/Yaseen-Anwar/3D-factory`)
**Range:** `13cdd07..f9d0c5f` — **50 commits**, 2026‑07‑30 → 2026‑08‑11
**Base:** `13cdd07` "3D Factory: import perf, cut fixes, FBX, 3-axis rotation, recorder" — this is
also the tip of `yaseen/main`, so **`main` has not moved since we branched.**

> Written for the team integrating this alongside their own work. It is ordered by what will
> actually cost you time: the four things that break a build come first, the conflict map second,
> and the commit list last. Every number here was measured on this tree, not estimated — if a
> figure below does not match what you see after merging, something was lost in the resolution.

---

## 0. TL;DR

| | |
|---|---|
| Commits to merge | 50 |
| Files added | 99 — 31 `.rs`, 5 packaging/docs, 63 assets (58 via LFS) |
| Files modified | 19 source files across 6 crates |
| Where the conflicts will be | `cad_app/src/app.rs` — 13,402 added lines in **274 hunks** |
| Public API breaks | **3** (all compile errors, all one-line fixes) |
| Silent behaviour changes | **1** (`cad_light::extrude`/`bbox` now return metres) |
| On-disk format changes | `.rsm` v7→v8, DXF `$INSUNITS`, `.simlux.json` new fields — all back-compatible |
| Test baseline after merge | **1006 tests, 0 failures** across the workspace |

```bash
git lfs install                      # REQUIRED — see §1
git fetch yaseen
git checkout main
git merge --no-ff yaseen/fresh-main  # merge, do not rebase — see §2
cargo test --workspace               # expect 1006 passed, 0 failed
```

---

## 1. Before you start: Git LFS is mandatory

The bundled assets (CC0 furniture, textures, door handles, apertures — **16 MB**) are tracked with
**Git LFS**, configured in `.gitattributes`:

```
assets/**/*.{fbx,obj,3ds,glb,gltf,bin,jpg,jpeg,png,exr}  filter=lfs diff=lfs merge=lfs -text
```

**Without `git lfs` installed, the merge appears to succeed and the app builds.** You will not get
an error. What you get instead is 130-byte text pointer files where the meshes should be, and every
library in the app reports "nothing to offer" — empty Furniture, Apertures, Handles and Textures
menus, with no message explaining why. That failure mode has already cost this project a day.

```bash
git lfs install
git lfs pull
git lfs ls-files | wc -l   # expect 58
```

`cad_app/src/assets.rs::report()` prints one line on stderr at startup naming what it found and
where — check it after the merge:

```
[assets] root=…\assets apertures=3 handles=3 cc0/textures=8 cc0/furniture=16
```

Any `MISSING` there means LFS did not resolve.

---

## 2. Merge, do not rebase

Use `git merge --no-ff`. Rebasing this branch is a trap: `cad_app/src/app.rs` has **274 hunks**
across 50 commits, and a rebase replays every one of those conflicts against every intermediate
commit — including `537c3fd`, which is **reverted by the very next commit** (`634eb7b`) and would
have to be conflict-resolved twice for no benefit. One merge resolves each region once.

If your policy forbids merge commits, squash-merge instead. Do not rebase.

---

## 3. The four things that will break your build

### 3.1 `cad_kernel::Document` gained a field — E0063

```rust
pub struct Document {
    …
    pub units: DocUnits,   // NEW
}
```

Any **struct-literal** construction of `Document` fails to compile:

```
error[E0063]: missing field `units` in initializer of `Document`
```

The break is narrower than it looks: this whole tree contains **exactly one** struct-literal
construction of `Document` (`cad_io/src/rsm.rs:655`, the loader), and it was updated. Everything
else goes through `Document::default()` and is unaffected. Yours will be in the same kind of place —
a custom loader, or a test helper. **Fix:** add `units: DocUnits::default()`, or
`..Default::default()`. `DocUnits::default()` is 1.0 m/unit with `source: Assumed`, i.e. exactly
the old behaviour.

### 3.2 `cad_kernel::Command` gained 5 variants — E0004

`Units(Option<f64>, bool)`, `Diag`, `Dedupe`, `RepairCuts`, `Scene`.

The enum is not `#[non_exhaustive]`, so any exhaustive `match` on it stops compiling. This already
bit `cad_cli/src/main.rs`, which is the canonical example of the fix (add the variants to the
existing catch-all arm). If you have your own dispatcher, expect one conflict there.

### 3.3 `cad_solid::Primitive` gained a variant — E0004

```rust
Sweep { profile: ProfileId, path: PathId, bmin: [f32; 3], bmax: [f32; 3] },
```

Same shape of break, same fix. Evaluated in `cad_solid/src/csg.rs`.

### 3.4 `cad_light::extrude`, `extrude_handles` and `bbox` return METRES now

**This one does not produce a compile error.** The signatures are byte-identical:

```rust
pub fn extrude(doc: &Document, height: f32) -> Vec<Mesh>
pub fn extrude_handles(doc: &Document, handles: &[u64], height: f32) -> Vec<Mesh>
pub fn bbox(doc: &Document) -> Option<(f32, f32, f32, f32)>
```

They now multiply plan coordinates by `doc.units.metres_per_unit` before building geometry, because
the lighting engine's world is metres and a millimetre plan was building a room 1000× too big.

**Why this is safe in practice:** the default is `metres_per_unit = 1.0`, so for any document that
has never declared a unit the output is bit-identical to before. The change only bites a drawing
that declares mm/cm/inch — and for those, the old answer was wrong. If you call these functions,
audit whether your caller was compensating for the old drawing-unit output; if it was, remove the
compensation rather than reverting this.

---

## 4. Conflict map, by risk

### Tier 1 — `cad_app/src/app.rs` (expect real work here)

13,402 insertions, 707 deletions, **274 hunks**, in a file that is now 50,279 lines. Be honest with
your estimate: this is **not** one appended block.

| hunk size | count | what they are |
|---|---|---|
| ≤ 5 lines | **161** | scattered one- and two-line edits — struct fields, match arms, menu entries, call sites |
| 6–50 lines | 79 | small handlers and UI blocks |
| > 50 lines | 34 | whole new subsystems, self-contained |

The 161 small ones are the risk, and they cluster in six regions. If your work touches any of these
line ranges, look there first:

| region | approx. lines | small hunks | what we added |
|---|---|---|---|
| Factory menus & dialogs | 5000–7500 | 35 | apertures, architecture, textures, scene-import menus |
| Sidecar save / load | 25000–27500 | 29 | new persisted state (see §5) |
| 3D viewport + panels | 10000–12500 | 21 | SIMLUX 3D panel, factory viewport handlers |
| Furniture / mesh import | 7500–10000 | 16 | FBX, glTF, texture binding |
| Painters & helpers | 47500–50000 | 13 | 2D overlays, unit conversion helpers |
| `CadApp` struct + `Default` | 2500–5000 | 11 | new fields |

**Named seams** — the places any feature must edit, and therefore where two teams collide:

| seam | line | note |
|---|---|---|
| `pub struct CadApp` | 1332 | **239 → 306 fields** (+67); take both sides, order is irrelevant |
| `impl Default for CadApp` | 3501 | must stay in sync with the above, field for field |
| `fn run_command_inner` | 12174 | command dispatch; our 5 new arms are at 14076–14230 |
| `fn update` | 35695 | the frame; the canvas `CentralPanel` starts ~37820 |
| `fn render_menu_flyouts` | 34535 | menu bar |
| `mod` list | `main.rs` 1–36 | +17 modules; a conflict here is trivially "take both" |

**Resolution rule for all of the above: take both sides.** Struct fields, `Default` initialisers,
`mod` declarations, match arms and menu entries are additive by nature. A conflict in these regions
is git failing to see that two people appended to the same list — it is not a semantic clash. The
only ones worth reading carefully are conflicts inside `fn update`'s canvas block, where ordering
between input handlers genuinely matters.

### Tier 2 — shared crates (small, but read every one)

| file | + / − | what changed | your action |
|---|---|---|---|
| `cad_kernel/src/document.rs` | +84 | `DocUnits`, `UnitSource`, `Document::units` | §3.1 |
| `cad_kernel/src/parser.rs` | +61 | 5 new commands | §3.2 |
| `cad_kernel/src/lib.rs` | +1/−1 | one re-export line | take both names |
| `cad_io/src/rsm.rs` | +88 | format v8 | §5.1 |
| `cad_io/src/dxf.rs` | +150 | `$INSUNITS` read/write | §5.2 |
| `cad_light/src/extrude.rs` | +56/−22 | unit scaling threaded through | §3.4 |
| `cad_light/src/lib.rs` | +2 | `pub mod ldt` + re-export | take both |
| `cad_solid/src/lib.rs` | +402 | `Path`/`PathId`, `Model::paths`, `rescale`, 11 new modules | mostly additive |
| `cad_solid/src/csg.rs` | +41 | evaluate `Sweep` | §3.3 |
| `cad_cli/src/main.rs` | +2/−1 | new command variants | §3.2 |
| `cad_app/Cargo.toml` | +7/−2 | see below | merge both dep lists |

**New dependencies** (`cad_app` only — no new workspace members):

```toml
image  = { version = "0.25", features = [… , "hdr", "exr"] }   # features ADDED to an existing dep
arboard = "3"      # OS clipboard → paste-as-texture (already in tree via egui-winit)
base64  = "0.22"   # texture blobs in the sidecar
```

If you also edited `cad_app/Cargo.toml`, the conflict is a union of two dependency lists. Keep the
added `hdr`/`exr` features on `image` — image-based lighting cannot work without them.

`cad_app/build.rs` is **new**: it stamps `SIMLUX_BUILD` from `git rev-parse --short HEAD`, with
`+dirty` when the tree is modified, and the binary prints it at startup. It fails soft when git is
absent, so it will not break a CI build from a tarball.

### Tier 3 — files we created (no conflict possible)

31 new `.rs` files plus packaging and docs. A conflict here means you happened to create a file
with the same path, which is worth knowing about but is not a merge problem:

```
cad_app/         build.rs
cad_app/src/     assets.rs color.rs door_mat.rs env.rs env_map.rs handles.rs matball.rs
                 material_graph.rs mesh_preview.rs pathtrace.rs pathtrace_gpu.rs proc_tex.rs
                 radiance_export.rs render_probe.rs report_figs.rs solar.rs texture_set.rs
cad_light/src/   ldt.rs
cad_solid/src/   architecture.rs cabin.rs couch.rs cupboard.rs desk.rs dogleg.rs door.rs
                 kitchen.rs meshcut.rs spiral.rs sweeplight.rs
cad_solid/tests/ architecture_tests.rs
packaging/       build-package.ps1 Install.ps1 README.txt
(root)           WORK_PATH.md .gitattributes   assets/cc0/README.md
```

`render_probe.rs` and `report_figs.rs` are `#[cfg(test)]`-only and add nothing to the shipped
binary: the first renders the reference scene headlessly to a PNG so a change to the *look* can be
judged by looking, the second renders a design document's figures from the code they document.

---

## 5. On-disk formats — all back-compatible, one rule each

### 5.1 `.rsm` native format: version 7 → 8

v8 appends a units block (f64 + u8 tag) after the version word. The writer is **conditional**:

```rust
fn version_for(doc: &Document) -> u16 {
    if doc.units.source == UnitSource::Assumed { 7 } else { 8 }
}
```

So a drawing that never declared a unit is still written as **v7** and still opens in an older
build. Only a drawing the user explicitly gave a unit becomes v8. The reader accepts both.

**If you also bumped the version number, this is a genuine semantic conflict** — resolve it by
allocating distinct version numbers and making the reader handle both blocks, not by picking one
side. This is the only place in the merge where "take both" is the wrong instruction.

### 5.2 DXF

`$INSUNITS` is now read from and written to the header. An imported plan carries its own scale
instead of being assumed to be metres. Files without the variable behave exactly as before.

### 5.3 `.simlux.json` sidecar

Every new field carries `#[serde(default)]`, so an old sidecar loads into a new build and a new
sidecar loads into an old one (the unknown fields are ignored). New in `SimluxConfig`:

- `factory: FactoryDoc` — the whole 3D model: solids, walls, storeys, furniture library and
  instances, per-feature and per-surface colours and textures, groups
- `luminaires: Vec<Luminaire>` + `next_luminaire_id` — the lighting layout
- `wall_centerline` — wall-style → linetype

Note the sidecar is gitignored (`*.simlux.json`) — it is per-drawing user state, not source.

---

## 6. Verifying the merge

Run these in order. The numbers are what this tree produces today; a drop means something was lost
in the resolution.

```bash
cargo build --workspace          # 0 errors
cargo test  --workspace          # 1006 passed, 0 failed, 32 ignored
cargo build --release -p cad_app # ~2 min; LTO is on for release
```

Per-crate baseline, so you can localise a failure:

| crate | tests | | crate | tests |
|---|---|---|---|---|
| `cad_app` (bin `simlux`) | 560 | | `cad_light` | 12 |
| `cad_kernel` | 194 | | `cad_nurbs` | 7 |
| `cad_solid` | 163 | | `cad_wall` | 6 |
| `cad_io` | 34 | | `cad_raster` | 6 |
| `cad_param` | 22 | | `cad_snap` | 2 |

Then five minutes of manual checks that no test covers, because they depend on assets and a GPU:

1. **Startup line** — `[assets] root=… apertures=3 handles=3 …`, no `MISSING` (proves LFS).
2. **▼ Furniture → Import** lists the CC0 library and a model appears (proves LFS binaries).
3. **▼ Apertures** — draw a window on a wall; the cut goes through (the single most-repaired area
   in this branch; commits `8b5523c`, `23d1570`, `68ed067`, `7341c95`).
4. **SIMLUX workspace** — the 3D panel shows the Factory model, and `⚡ Calculate` returns a lux
   figure. Import an `.ies` or `.ldt` and place a point on the plan.
5. **`units mm`** in the command line, then build — the model comes out at true size (§3.4).

If you want a deeper check, the session recorder captures everything: type `dbg` (or `recorder`) in
the command line to open it, and `scene` for a measured dump of the 3D state — numbers to compare,
rather than judging a 3D result from a screenshot.

---

## 7. If you want only part of this

The 50 commits are not equally entangled. Ranked by how cleanly each group lifts out:

| group | commits | separable? |
|---|---|---|
| **Parametric generators** (`cad_solid`: door, cupboard, kitchen, cabin, desk, couch, stairs, spiral, sweeplight) | 3 + part of `f1d2731` | **Yes** — new files + one `pub mod` line each, plus their `▼` menu entries |
| **Photometry: EULUMDAT** (`cad_light/src/ldt.rs`) | 1 | **Yes** — new file, 2 lines in `lib.rs` |
| **Packaging / installer** (`packaging/`, `assets.rs`, `build.rs`) | 1 | **Yes** — self-contained, and it fixes a real bug (CWD-relative asset loading) |
| **Renderer** (HDR env, colour management, TAA, shadows, SSGI, SSR, path tracer) | 8 | Mostly — confined to `light3d.rs`, `color.rs`, `env*.rs`, `pathtrace*.rs`, but wired through `app.rs` menus |
| **Units** | 4 | **No** — spans `cad_kernel`, `cad_io`, `cad_light`, `app.rs` by design. All or nothing. |
| **Cut / opening repairs** | 9 | **No** — later commits fix earlier ones; taking a subset reintroduces fixed bugs |
| **SIMLUX lighting workflow** | 4 | Depends on Units |

Two commits deserve a warning if you cherry-pick:

- **`537c3fd`** "Cuts: bind the opening to the wall it was drawn on" — **do not take this alone.**
  It deleted ~30% of a real building's geometry (7485 → 5261 triangles, measured). It is reverted
  by the very next commit, `634eb7b`.
- **`2123aac`** "Villa autoload: apply a fixed x35 scale" — a dev render fixture that loads a
  134 MB glTF from a **hardcoded local path** (`G:\blender dev\…`, `app.rs:12`). It is opt-in and
  **off by default**, and skips silently when the file is absent, so it cannot affect anyone else's
  machine — but you may simply want to delete the constant and its two call sites.

**Two environment variables** exist for development, both off unless set:

| var | effect |
|---|---|
| `SIMLUX_VILLA=1` | auto-load the villa render fixture on startup (see above) |
| `SIMLUX_REPAIR=<drawing>` | open that drawing, run the opening repair once, report, and do **not** save |

Neither runs in a normal session. `SIMLUX_REPAIR` exists because the repair was twice run against
a stale binary, where it reported "no shallow cuts found" — indistinguishable from success.

---

## 8. Appendix — the 50 commits, by theme

**Renderer and materials** (8)
`f1d2731` Materials Factory, path tracing (CPU+GPU), daylight, generators ·
`bf3688d` Materials Factory UX + Radiance output ·
`6e13fe9` HDR environments, colour management, temporal accumulation, cascaded shadows, SSGI ·
`15b3df4` SSGI: stop applying the receiver's cosine twice ·
`c07fc19` Screen-space reflections, transmission, square-on face view ·
`90c97ee` A stable tie-break so coincident faces stop flickering ·
`563618e` Depth precision: raise the near plane, size the tie-break ·
`8c4ba41` Unstable HashMap order was resetting TAA every frame

**Precision fixes at survey coordinates** (2)
`c6d86e0` Rebase texture coordinates: the moiré was UV precision, not depth ·
`2d20979` Rebase the shader's world position per VERTEX, not per fragment
> At 6852 m one f32 ULP is 0.82 mm. Four shaders were differentiating an un-rebased world position;
> `dFdx` of a quantised large value is catastrophic cancellation, and a 5th-power Fresnel turns that
> into 0.04↔1.0 swings — the black speckle on glass. A test now forbids the whole family.

**Glass and transparency** (1)
`63fa56b` Build the pane normal from a vertex position, not from depth

**Parametric generators** (3 standalone; the rest arrived inside `f1d2731`)
`62fd14f` sweep light · `8e35d7b` office desk · `526223e` couch · door, cupboard, kitchen, cabin
and staircase/spiral/ramp landed inside `f1d2731`

**Units** (4)
`420ad63` explicit drawing scale · `8d0aff5` the reverse 3D→2D direction ·
`2de7bb2` DXF `$INSUNITS` · `56f3cac` `units <unit> rescale`

**Cuts and openings** (9)
`8b5523c` three faults in the thickness probe · `0e220e1` find the wall, don't trust the sketch
plane · `537c3fd` (**reverted**) · `634eb7b` the revert · `8f8d0dc` judge the repair by the solid,
not by its own rule · `23d1570` measure across the whole opening · `68ed067` choose targets from
the whole opening · `7341c95` repair by coverage · `c99e328` `repaircuts`
> `8f8d0dc` is the guard worth understanding before touching this code: a repair that costs the
> solid a tenth of its triangles is abandoned. It exists because `537c3fd` was "verified" with the
> same heuristic that performed the move, so the check could only ever report success.
> Net effect on the reference building: **28 → 34 of 40 openings** cut fully through. The remaining
> 6 are deliberately untouched — 2 are 99 mm short and indistinguishable from a recess, 4 are 2.14 m
> from any wall and need redrawing.

**Diagnostics** (10)
`1f77862`, `274d15c`, `9c5c7a6`, `21cce19`, `da5f7d7`, `a5ec15c` (`diag`) ·
`d3dbd82` (`dedupe`) · `238ce7a`, `17433c3`, `56e4694` (recorder: 3D scene capture)

**Data-loss fixes** (3) — take these regardless of what else you take
`273d0ad` Opening a file must drop the undo history — **this destroyed a project** ·
`933f1f5` Autosave is OFF at every start, and never remembered ·
`58a782c` Fix autosave overwriting the drawing with an open face-sketch

**2D plan** (2)
`fa61612` furniture outlines with a FURN badge · `a03d4dd` let solids hide the underlay

**Shipping** (2)
`2f6c8a9` resolve assets from the executable, and package for install · `4eea41d` `SIMLUX_REPAIR=`
and stamp the binary with its commit

**SIMLUX lighting** (4)
`d9863de` light the real building, read EULUMDAT · `e405b06` Factory-style toolbar + array tool ·
`e23e998` mount each fixture to the ceiling above it · `f9d0c5f` movable light points + fittings
library

**Docs** (2)
`6535459` record the renderer/furniture/autosave/units work · `2123aac` villa autoload fixture

---

## 9. Questions

The three areas where a wrong resolution is expensive, in order:

1. **`.rsm` version number** (§5.1) if you also bumped it — the only true semantic conflict.
2. **`cad_light::extrude`/`bbox` returning metres** (§3.4) — silent, no compile error.
3. **Ordering inside `fn update`'s canvas block** — input handlers gate each other; conflicts there
   need reading, not "take both".

Everything else in this merge is additive.
