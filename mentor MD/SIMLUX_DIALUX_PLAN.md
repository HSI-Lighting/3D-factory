# SIMLUX → a functional DIALux, in Rust — Plan for the Coding Agent

**Status:** living document · **Maintainer:** supervisor/mentor (not a code author) ·
**Last set:** 2026-07-09

This is the strategic plan the coding agent works from. It records the product
direction, the two **locked decisions**, the architecture that keeps those
decisions from stalling the build, and the phased path. It is maintained by the
supervisor as commits land — treat it as the source of truth for *what to build
next and why*. The **scene / render / daylight** subsystem (import Blender
objects, furniture, materials, render settings, sun path, daylight diagram) has
its own companion: **`SIMLUX_SCENE_AND_DAYLIGHT_PLAN.md`** (expands §9 + the v0.4
daylight frontier). Companion docs: `SIMLUX_LUX_WORKFLOW.md` (pipeline),
`SIMLUX_STATUS.md` (what's done), `3D_LIBRARY.md` (csgrs), `Accuracy_Test_Plan.md`
(validation), `AGENTS.md` (the 21 coding rules — still binding).

---

## 0. The reframe (read this first)

The goal is **a functional DIALux in Rust**. Do **not** interpret that as "clone
DIALux the application" — that target is unbounded (catalogs, radiosity, daylight,
UGR, road/emergency/sports modes, photoreal render, PDF documentation: ~20 years
of work). Chasing the app never converges.

**Build to the *standard DIALux implements*, not to the app.** DIALux's real job
is proving a room meets **EN 12464-1** (indoor workplace lighting): task-area mean
illuminance, uniformity, glare (UGR), colour rendering (Ra), energy. That standard
is a *finite checklist*. Build the checklist → you get a functional DIALux for the
80% case: **a standards-compliant indoor illuminance calculator + report.**

**Key insight:** the gap to DIALux is ~**80 % product-surface** (photometry
formats, metrics, zoning, reporting, arrangement tools) and ~**20 % solver**.
SIMLUX already has a credible engine (IES + ray-traced direct + Monte-Carlo
indirect + reflectances, 6 passing tests). **Invest in the product surface; do
not rewrite the physics** except where a locked decision says so (radiosity, below).

---

## 1. Locked decisions (2026-07-09)

### Decision 1 — Target standard = **Both, EN-first**
- Build the **EN 12464-1** framework first: task area + immediate surrounding
  area + background, with target rows per application type
  (E̅m / U₀ / UGR_L / Ra / energy).
- Keep **IES LM-63** as an **equal** input format alongside **EULUMDAT (`.ldt`)**.
- **ANSI/IESNA** criteria come later as an *alternate ruleset* over the same
  computed metrics — not a parallel engine.

### Decision 2 — Indirect calculation = **Hybrid (raytrace direct + radiosity indirect)**
- Ray-trace the **direct** component (already exists).
- **Radiosity** for the interreflected (indirect) component — deterministic,
  reproducible report numbers, matches DIALux methodology.
- **Synergy:** radiosity yields **surface luminance (cd/m²)** for free, which is
  exactly the input **UGR** needs. The radiosity build and the glare metric are
  **one investment** — sequence them together.

---

## 2. Architecture — three seams carry everything

Every ambitious choice goes **behind an interface**, and the working code stays
the default until the new code out-validates it. This is what makes Decision 1 +
Decision 2 survivable.

| Seam | v1 behind it (default) | Added later behind it | Why it saves you |
|------|------------------------|-----------------------|------------------|
| **`Photometry`** | IES LM-63 (exists) | EULUMDAT `.ldt` | Both parsers emit **one** internal C-γ intensity table. Adding LDT never touches the calc. |
| **`IndirectSolver`** | Monte-Carlo (exists) | Radiosity | Radiosity is built + validated in parallel; flip the default only on validation parity. Product never regresses. |
| **`ComplianceStandard`** | EN 12464-1 | ANSI/IESNA | Metrics (E̅, U₀, UGR, Ra) computed **once**; the standard only *judges* them. IESNA is a table, not a rewrite. |

**Rule:** no feature is allowed to reach into a concrete implementation past its
seam. UI and calc talk to the trait, never to `Ies` / `Radiosity` / `En12464`
directly.

---

## 3. The radiosity build — guardrails (the schedule risk lives here)

Radiosity is the single heaviest item. It sinks schedules when done carelessly.
Non-negotiable rules:

1. **Never replace MC in place.** Implement `Radiosity` as a *second*
   `IndirectSolver`. Monte-Carlo stays the shipping default until radiosity passes
   the same validation cases (§6). Then flip the default. No big-bang swap.
2. **Reuse the BVH.** The existing ray tracer (`cad_light/src/rt.rs`) already does
   occlusion — that is exactly the visibility test form-factor computation needs.
   Do **not** build a second visibility engine.
3. **Do not double-count the direct component.** Direct illuminance seeds the
   initial patch radiosities; interreflection then redistributes it. The final
   work-plane grid must add direct **once**. Adding it again after the radiosity
   solve is the classic hybrid bug — every number reads high. Put this on the PR
   checklist.
4. **Mesh sensibly.** Patch subdivision drives both accuracy and cost. Start
   coarse, refine adaptively near high-gradient regions. Log the patch count so
   runaway meshing is visible (cf. `AGENTS.md` rule 11 — overflow warnings).
5. **Stay off the UI thread.** A radiosity solve is a background op — follow
   `Background_Ops_Pattern.md` (pure on worker, apply on main, cancel drops result).

---

## 4. Phased roadmap

### v0.1 — "Compliant single room" (product, not physics)
- Finish the pipeline: **slices 3–4** — LUX block definition with **type-level
  IES** (decision D4) → **derive `Luminaire`s** from block instances (replace the
  `LightState.luminaires` side-list). *Substrate already shipped (block/insert).*
- **LDT parser** into the shared `Photometry` type.
- **Layer→3D dialog** (§5).
- **Maintenance factor** (a multiplier — quick win, real standard requirement).
- **Task / surrounding-area zones** + **EN 12464-1 compliance verdict**
  (E̅m / U₀ vs target rows).
- *In parallel, behind the trait:* begin the **radiosity solver + its validation
  harness**. No product feature waits on it.

### v0.2 — the moment it reads as DIALux
- Flip default to **radiosity** once validated.
- **UGR** (uses surface luminance from radiosity) · **luminance false-colour** ·
  **isolux contour** lines · a **report / PDF** deliverable (title, fixtures, IES
  names, zone stats, pass/fail).

### v0.3 — breadth
- Luminaire **arrays / aiming** (line / field / circle, tilt + rotation).
- **Energy** metrics (LENI, W/m²/100lx, power density).
- **GLDF/ULD** manufacturer catalogs (extends the `Photometry` seam).

### v0.4+ — the hard frontier
- **Daylight**: windows, CIE sky models, daylight factor.
- **Photoreal** render — see **§9** (Blender-quality via Cycles).
- Interactive-3D editing viewport via `cad_solid` (csgrs) — see `3D_LIBRARY.md`.

---

## 5. Layer → 3D dialog (spec)

Matches resolved decisions **D1** (room source = a chosen layer) and **D2**
(per-layer height). The SIMLUX section opens a **layer table**, one row per
document layer:

| Column | Values |
|--------|--------|
| ☑ Use for 3D | on / off |
| **Role** | Wall · Floor · Ceiling · **Opening (void)** · Obstruction · ignore |
| Height | base Z → top Z (mm) |
| Reflectance / material | ρ value, from the material library |

- **OK** captures **handles** per layer group (handle-stable — survives edits,
  Phase B3) and extrudes each group at its own height.
- **Role = Opening** → the layer's geometry becomes a **boolean void** cut from
  the host wall solid (csgrs / `cad_solid`, `3D_LIBRARY.md`). This is how
  windows/doors get modelled.
- Reflectance-per-layer feeds the calc; per-role defaults are fine.

---

## 6. Validation is GATING

A lighting engine nobody trusts is a demo. Wire `Accuracy_Test_Plan.md` into CI:

- **Analytical checks** — point source → inverse-square; infinite uniform diffuse
  surface → closed form.
- **CIE 171:2006** test cases — the industry validation suite. This is the bar
  radiosity must clear before it becomes the default (§3.1).
- **Cross-check** identical scenes against DIALux itself; record the delta.

**No compliance feature ships without a validation number attached.** That is the
line between "a functional DIALux" and "a heatmap that looks like one."

---

## 7. Guardrails that stay binding

- New work in **independent crates**; `cad_kernel` / `cad_io` stay **untouched**
  (SIMLUX state lives in the `drawing.simlux.json` sidecar).
- All 21 rules in `AGENTS.md` (GPU-only render, undo per op, no silent failures,
  hot-path performance, …).
- Push target: **dokkandar `origin`** only. HSI upstream is read-only.

---

## 8. Do next (crisp)

1. Land **slices 3–4** (LUX block + derive luminaires).
2. Add the **LDT parser** and the **layer→3D dialog** (with Role column).
3. Stand up **EN 12464-1 zones + verdict** and the **maintenance factor**.
4. Start **radiosity + validation harness** in parallel, behind `IndirectSolver`,
   MC still default.

Everything above the frontier (v0.4) waits until v0.1–v0.3 are real and validated.

---

## 9. Rendering & Blender-object import (Blender-quality visualization)

**User requirement (2026-07-09):** keep the *technical* parts — 3D modeling, IES
photometry, lux calculation — **independent of Blender**, but for **render,
object import, materials, and scene settings**, get **Blender-quality** results.

### 9.1 The rule that keeps this safe — two engines, never conflated
- **`cad_light` = the calculation of record.** SIMLUX's own engine produces every
  number that matters (lux, UGR, EN 12464-1 verdict). It stays 100 % independent
  of Blender — this satisfies "technical parts independent from Blender."
- **Cycles = presentation render only.** A beauty path-trace is a *picture*, never
  a compliance result. Even though Cycles is physically based and parses IES, its
  output must never be reported as a photometric number. (DIALux keeps
  *calculation* and *rendering* as separate deliverables — do the same.)
- **Consequence:** the render back-end is **optional and quarantined**. If it (or
  Blender) is absent, SIMLUX still does its whole job — you just don't get the
  pretty image. This is what contains the licensing/heavy-dependency risk.

### 9.2 The license landmine — Blender is GPL, Cycles is Apache-2.0
- **Blender itself (and EEVEE) = GPL.** Never link Blender code into SIMLUX; it
  would force the whole product to GPL. This violates the permissive-only policy.
- **Cycles = Apache-2.0**, relicensed in 2013 *specifically* to be embedded in
  other open-source and commercial software. Cycles is the render engine to use.
  It has native **IES** support and already embeds in other DCCs (Houdini via
  hdCycles), so the path is proven.

### 9.3 Which Blender modules matter (and which to avoid)
| Module | Use it? | Role |
|--------|---------|------|
| **`intern/cycles`** | ✅ **THE module** (Apache-2.0) | the renderer |
| `intern/cycles/app` | ✅ | standalone app + **XML scene loader** — the simplest integration surface (write scene XML → render) |
| `intern/cycles/scene` | ✅ | `Scene`/`Mesh`/`Object`/`Shader`/`Light`/camera + **native IES** (`ies.*`) |
| `intern/cycles/session` | ✅ | the `Session` render driver |
| `intern/cycles/device` | ✅ | CPU / CUDA / OptiX / HIP / Metal back-ends (GPU render) |
| `source/blender/io/*` | ⚠️ only via the Blender-process route | USD / Alembic / Collada / OBJ / STL importers; glTF is the `io_scene_gltf2` addon |
| `source/blender/draw/engines/eevee_next` | ❌ | EEVEE — GPL + welded to Blender's GPU/draw manager; **not extractable** |
| rest of `source/blender` | ❌ | the GPL application; never link |

For a realtime GPU *preview* (not final render) use the existing glow renderer or
a pure-Rust engine (e.g. `rend3`) — **not** EEVEE.

### 9.4 Practical approach — phased
- **Phase 1 (pragmatic, recommended first): Blender headless as an external
  subprocess.** Reuse the existing external-converter pattern (`tools/dwgconv`).
  SIMLUX exports the neutral scene → **glTF/USD** + a render script; runs
  `blender --background --python render.py`; reads back the **EXR/PNG**. Zero
  linking (separate process ⇒ no GPL entanglement), minimal code, real Cycles
  quality. Cost: requires Blender installed (acceptable for an optional
  "photo render" feature).
- **Phase 2 (deeper, optional): embed Cycles standalone (Apache-2.0)** via a thin
  **C-ABI wrapper crate** (`cad_render`), same FFI discipline as csgrs/dwgconv.
  In-app rendering, no Blender install needed. Cost: build Cycles + translate the
  neutral scene → Cycles nodes; note the standalone app is officially "work in
  progress," so budget integration effort.
- **Phase 3 (only if you outgrow both): USD + a Hydra render delegate**
  (hdCycles). Renderer-agnostic, but USD is a heavy C++ dependency — defer unless
  multi-renderer support becomes a real requirement.

### 9.5 The fourth seam — `Renderer`
Consistent with §2: SIMLUX's **neutral scene** (rooms + fixtures + imported meshes
+ materials + camera + environment) is the source of truth; **"Cycles/Blender" is
one back-end behind a `Renderer` trait.** This is what literally keeps the
technical core independent of Blender — the render back-end is swappable.

### 9.6 Object import
- **Format = glTF 2.0** (Rust `gltf` crate, MIT/Apache): carries mesh **+ PBR
  materials**, permissive, Rust-native. Blender-native `.blend` is only read well
  by Blender, so "import Blender objects" means the user exports **glTF** (or the
  Blender subprocess converts `.blend → glTF`).
- An imported object becomes an **Obstruction-role** mesh (§5): it **casts shadows
  in the `cad_light` calc** *and* renders with full material in the beauty pass —
  one asset, both engines.

### 9.7 Materials & scene settings
- **Material model = Principled BSDF ↔ glTF metallic-roughness ↔
  UsdPreviewSurface** (mutually mappable). SIMLUX stores a **neutral PBR
  material**; the exporter maps it to Cycles' Principled BSDF (or writes glTF/USD
  that Cycles reads natively). This is why glTF is the sweet-spot interchange —
  materials survive the round-trip.
- **Scene settings** = a neutral `RenderScene` struct: camera (pos/target/FOV),
  environment (HDRI/world), sun/sky, exposure, sample count. Exported to the
  back-end script/XML.

### 9.8 Pipeline (Phase-1 recommended)
```
SIMLUX neutral scene ──► export glTF/USD + render.py
   (rooms, IES fixtures,        │
    imported meshes,            ▼
    PBR materials, camera)  blender --background --python render.py   [Cycles, Apache-2.0]
                                │
                                ▼
                          EXR/PNG  ──► display in SIMLUX (presentation only)

  cad_light (independent)  ──► lux / UGR / EN 12464-1 verdict   [the numbers of record]
```
