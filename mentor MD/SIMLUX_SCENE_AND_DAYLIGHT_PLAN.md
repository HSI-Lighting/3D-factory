# SIMLUX — Scene, Render & Daylight (Artificial + Natural Light) — Plan for the Coding Agent

**Status:** living document · **Maintainer:** supervisor/mentor (not a code author) ·
**Set:** 2026-07-09

This doc governs the **lighting-scene subsystem**: the *artificial-light* side
(import Blender objects — furniture, fixtures, materials — plus render + render
settings) and the *natural-light* side (**sun path, daylight diagram, daylight
simulation**). It is the near-term overview + the binding rules for that work.

Read alongside: `SIMLUX_DIALUX_PLAN.md` (the master plan — this doc expands its
§9 render section and v0.4 daylight frontier), `SIMLUX_LUX_WORKFLOW.md`,
`3D_LIBRARY.md`, `AGENTS.md` (21 rules — still binding).

---

## 0. Scope

A lighting app needs a **scene**, not just a grid. Two families of light:

- **Artificial light** — luminaires (IES) + the *stuff in the room* (furniture,
  fixtures, materials) + a photoreal render of it.
- **Natural light** — the sun and sky: where the sun is (sun path), how it enters
  (windows), and how much daylight results (daylight factor / EN 17037).

Both feed the same room model and must obey the **one rule** below.

---

## 1. THE ONE RULE — calculation of record vs presentation (never conflate)

Repeated from `SIMLUX_DIALUX_PLAN.md` §9 because it governs *everything* here:

- **`cad_light` = the numbers of record.** Lux, UGR, daylight factor, EN 12464-1 /
  EN 17037 verdicts. Physically validated, Blender-independent.
- **Cycles/Blender render = presentation only.** A picture. Never a compliance
  number — even though Cycles is physically based and parses IES/sun.
- **Everything in this doc slots into one side or the other.** When a feature could
  go either way (e.g. "sun"), it produces *two* consumers of one shared input, not
  one blurred result.

---

## 2. Site & Sun — the shared source of truth

One model feeds **both** the daylight calc and the render. Add it SIMLUX-side
(sidecar; `cad_kernel` untouched):

```
Site {
  latitude, longitude, timezone,       // where on Earth
  north_angle,                         // true-north vs drawing +Y (orientation)
  ground_reflectance,                  // albedo
}
SunClock { date, time }  ·  or a range for studies (day / year)
```

- **Solar position** = f(Site, date, time) → **(azimuth, altitude)** → a sun
  direction vector. Behind a `SolarPosition` seam:
  - *Diagram / shadow study tier:* PSA or NOAA algorithm (arc-minute accuracy) —
    cheap, pure math, no deps.
  - *Compliance tier:* **NREL SPA** (Reda–Andreas, ±0.0003°) when daylight numbers
    must be defensible.
- The **same sun vector** drives (a) the daylight calc's direct-sun component,
  (b) the render's sun light, and (c) the sun-path diagram. Compute once, consume
  three ways.

---

## 3. Artificial-light section

### 3.1 Imported objects — furniture / décor (glTF)
- **Format = glTF 2.0** (Rust `gltf` crate; mesh + PBR materials). `.blend` is only
  read well by Blender → user exports glTF, or the Blender subprocess converts
  `.blend → glTF` (`SIMLUX_DIALUX_PLAN.md` §9.6).
- An imported prop = an **Obstruction-role** mesh: it **casts shadows in the
  `cad_light` calc** *and* renders with full material. One asset, both engines.
- **Loader = the plain `gltf` crate** (+ `image` for textures). **NOT `bevy_gltf`**
  — it drags in the whole Bevy engine, against the lightweight/permissive policy.
- **COORDINATE CONVERSION IS MANDATORY.** glTF is **Y-up**; the SIMLUX engine world
  is **Z-up** (`SIMLUX_STATUS.md`: "Engine world is Z-up… IES nadir = −Z"). Apply
  the Y-up→Z-up rotation to every imported root transform. *(Do NOT skip this — a
  raw import lands every room on its side. A reviewed draft wrongly asserted the
  engine is Y-up; it is not.)*
- **Furniture tagging:** objects whose name/collection matches keywords
  (`chair`, `table`, `sofa`, `desk`, `furniture`, …) get a `furniture` flag for UI
  filtering. Cheap, pragmatic. *(Idea adopted from the reviewed browser-arch doc.)*
- **Why glTF, not raw `.blend`:** glTF gives the **evaluated** mesh (modifiers /
  geometry-nodes already applied) + PBR materials as a stable, versioned contract.
  Parsing `.blend` directly hits un-evaluated geometry, version-coupled DNA, and
  node-tree materials that need Blender's runtime to interpret — Blender itself
  recommends glTF/USD for interchange. See §10.

### 3.2 Imported lights — the two-representation rule (CRITICAL)
A luminaire has **two** representations; keep them distinct:

| Representation | Owner | Used by |
|----------------|-------|---------|
| **Photometric** — IES distribution + pose | the **LUX block** (type-level IES) | `cad_light` calc (the numbers) |
| **Visual** — the fixture body/lens mesh | imported glTF (optional) | the render only |

- A **Fixture** binds them: `{ lux_block (photometric identity), visual_mesh?
  (render geometry), emissive? }`, sharing position/rotation from the block
  instance.
- **HARD RULE:** an imported Blender light's *own* lamp data (a point/area light
  baked into the `.blend`) must **NEVER** feed the calc. Compliance light comes
  **only** from the assigned IES. The render may glow however it likes; the numbers
  come from IES. (Otherwise the render "looks lit" but the lux came from a stray
  Blender lamp — a silent correctness hole.)

### 3.3 Materials
- **Model = Principled BSDF ↔ glTF metallic-roughness ↔ UsdPreviewSurface**
  (mutually mappable). Store a **neutral PBR material** SIMLUX-side; the render
  exporter maps it to Cycles.
- The calc only needs **reflectance ρ** (already have it per surface/layer). The
  full PBR set (base colour, metallic, roughness, normal, emissive) is **render
  only**. Derive the calc's ρ from the material's diffuse albedo so the two stay
  consistent, but never let render-only channels leak into the calc.

### 3.4 Render + render settings
- Engine = **Cycles (Apache-2.0)**, via **Phase-1 Blender-headless subprocess**
  (`SIMLUX_DIALUX_PLAN.md` §9.4). Never link Blender/EEVEE (GPL).
- **`RenderScene`** (neutral, SIMLUX-side) = camera (pos/target/FOV), environment
  (HDRI / sky — see §4), sun (from §2), exposure/tone-map, sample count,
  resolution. Exported to the back-end script; result read back as EXR/PNG and
  shown as **presentation only**.
- Behind the **`Renderer` seam** (§6) so Cycles is swappable.

---

## 4. Natural-light section

Anchor daylight work to a **standard**, same discipline as EN 12464-1 for
artificial: **EN 17037 (Daylight in Buildings)** — daylight provision, **sunlight
exposure**, glare (DGP), view. Sunlight-exposure is a direct sun-path consumer.

### 4.1 Sun-path diagram  ← the first, self-contained deliverable
- A 2D projection (stereographic sky dome) showing the sun's trajectories:
  daily arcs for solstices/equinoxes, hour lines (analemmas), azimuth/altitude
  grid. Inputs: Site + date range. **No ray tracing** — just solar position +
  projection. Low-risk, high-value; ship it early.
- Bonus: orient it to the drawing's true north and overlay on the site plan.
- **Concrete tools:** the pure-Rust **`spa` crate** (NREL SPA — *verify MIT/Apache
  before adding*) behind the `SolarPosition` seam; draw the diagram with
  **`egui_plot`** (a separate crate in egui 0.30 — *not* `egui::plot`).

### 4.2 Shadow study
- Given the sun vector at date/time, project shadows of the extruded 3D massing;
  animate across a day / year. Reuses the existing 3D + a shadow projection (or
  the ray tracer). Mid-term.

### 4.3 Daylight calc (the numbers of record)
- **Apertures = windows.** They are the Role=**Opening** voids from the layer→3D
  dialog (`SIMLUX_DIALUX_PLAN.md` §5) — the entry points for sky + sun.
- Behind a **`SkyModel` seam**:
  - *Daylight factor* (classic metric): **CIE Standard Overcast Sky**
    (Lθ = Lz·(1+2 sinθ)/3). Diffuse only — no sun.
  - *Direct sun + clear sky:* CIE Clear Sky or **Perez all-weather** (needs
    irradiance inputs). Later.
  - *Climate-based (CBDM — sDA/ASE):* needs **EPW** weather files + annual runs.
    Far future.
- Sky/sun contribution enters the **same radiosity/raytrace engine** (Decision 2
  in the master plan) through the apertures. Daylight + electric light can then be
  summed for total illuminance.

### 4.4 Sun & sky in the render
- Render side: sun = a directional light + disc (from §2 sun vector); sky = a
  **procedural physical sky** environment. **Use a render-oriented analytic sky —
  Hosek–Wilkie (preferred) or Preetham** — generated to a lat-long HDR for
  image-based lighting. *(Model choice adopted from the reviewed browser-arch doc.)*
- **Two sky models, by side (do not cross them):** the *calc of record* uses the
  **CIE / Perez** sky (§4.3) — photometric, standards-aligned; the *render* uses
  **Hosek–Wilkie / Preetham** — perceptual, for a beautiful image. Presentation
  only; the daylight *numbers* come from §4.3, never from the render.

---

## 5. Data-model additions (all SIMLUX-side, sidecar — `cad_kernel` UNTOUCHED)
Extend `drawing.simlux.json` (serde) — decision D5:
- `Site` + `SunClock` (§2).
- Fixture bindings: LUX-block-def → { IES name, visual glTF path?, emissive? }.
- Imported-asset table: glTF path, transform, role (Obstruction/décor), material.
- Material library (neutral PBR).
- `RenderScene` presets (cameras, environment, exposure, samples).
- Daylight settings (sky model, weather file ref later).

---

## 6. Seams (reuse the trait discipline from the master plan)
Four seams already named (`Photometry`, `IndirectSolver`, `ComplianceStandard`,
`Renderer`). This subsystem adds two:
5. **`SolarPosition`** — PSA/NOAA (diagram) ↔ NREL SPA (compliance), swappable.
6. **`SkyModel`** — CIE Overcast ↔ CIE Clear ↔ Perez ↔ CBDM, swappable.

No feature reaches past a seam into a concrete impl.

---

## 7. Rules / guardrails (binding)
1. **Calc vs render never conflated** (§1). Render output is never a reported number.
2. **IES-only for the calc** — imported Blender lamps never feed compliance (§3.2).
3. **Never link Blender/EEVEE (GPL).** Cycles (Apache-2.0) via subprocess/standalone only.
4. **`cad_kernel` / `cad_io` untouched** — all new state in the sidecar.
5. **Render + daylight back-ends are optional + quarantined** — absent ⇒ SIMLUX
   still computes and reports; only the picture/daylight-extras are missing.
   Concrete mechanism: put **Cycles behind a Cargo feature gate** (`cad_render`
   optional feature) so the app builds and runs with no Cycles present.
6. **New work in independent crates** (`cad_render`, a `cad_daylight`/`cad_sky`?),
   behind seams. All 21 `AGENTS.md` rules hold.
7. **Validation gates the calc side** — daylight factor against analytical/known
   cases before it is reported (extend `Accuracy_Test_Plan.md`). The render side
   needs no numeric validation (it is not a number).

---

## 8. What happens — overview & phasing

### Very soon (near-term, low-risk, high-value)
- **Site & Sun model** + a `SolarPosition` impl (§2).
- **Sun-path diagram** (§4.1) — self-contained, no ray tracing.
- **glTF object import** (§3.1) as Obstruction meshes — furniture in calc + render.
- **Fixture two-representation** wiring (§3.2) — LUX block + optional visual mesh.

### Soon
- **Blender-headless render** (Cycles) of the scene (§3.4) + `RenderScene` presets.
- **Shadow study** across day/year (§4.2).
- **Daylight factor** via CIE Overcast Sky through window apertures (§4.3).

### Later
- Clear-sky / Perez direct sun; EN 17037 sunlight-exposure + DGP glare.
- Climate-based daylight (EPW, sDA/ASE); embedded Cycles standalone (`cad_render`).

---

## 9. Lands first (crisp)
1. `Site` + `SunClock` + `SolarPosition` seam.
2. **Sun-path diagram** panel.
3. glTF import → Obstruction meshes.
4. Fixture = LUX block + optional visual mesh (IES-only calc rule enforced).

Render and daylight *calculation* build on top of these; nothing here waits on the
radiosity work in the master plan — the sun-path diagram and import land against
today's engine.

---

## 10. Rejected alternative — in-browser `.blend` parser (recorded 2026-07-09)

A competing architecture proposed parsing `.blend` directly in the browser
(`blender-file`/`js.blend` + Web Workers + WebGL render, TypeScript). **Rejected.**
Reasons, so this is not re-litigated:

1. **Off-platform.** SIMLUX is a native **Rust** egui/glow app. That doc is a
   TypeScript/WebGL **browser** app — adopting it means discarding `cad_light`,
   `cad_kernel`, and the GPU renderer and reversing the pure-Rust-native decision.
   Confirmed to stay native Rust (user, 2026-07-09).
2. **Raw `.blend` is more fragile, not "deeper/solid."** It is Blender's internal
   memory dump, **not** an interchange format (Blender recommends glTF/USD/FBX).
   Direct parsing hits: un-evaluated modifiers/geometry-nodes (needs Blender's
   runtime), version-coupled DNA (JS parsers lag releases), and node-tree materials
   that degrade to a **grey placeholder** — whose own fallback re-summons a headless
   Blender, contradicting the "no Blender" premise.
3. **It silently drops "Blender-quality" render** — a hand-rolled WebGL rasterizer
   (EEVEE-lite preview), not Cycles path-tracing.
4. **It merges calc and render** into one result → no defensible photometric
   numbers. Fatal for a lighting-compliance tool (a *functional DIALux*).

**Kept from it:** Hosek–Wilkie/Preetham render-side sky (§4.4) and furniture-by-name
tagging (§3.1). Everything else superseded by glTF-interchange + Cycles + the
calc/render separation.
