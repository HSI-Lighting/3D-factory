# simLUX 3D roadmap — reach DIALux-style mesh creation, in 10 steps

**Target (owner, 2026-07-22):** match DIALux's 3D-modelling deliverable — a **watertight,
tessellated room MESH with per-surface reflectance**, handed to the light calc. Grounded in
[[LIGHTING_3D_STACK_RESEARCH_2026-07-22]]: DIALux/Relux/AGi32 all feed the calc **meshes +
reflectance**, never B-rep. So "3D modelling done" = *the room mesh the calc consumes.*

**Guiding rule from the research:** keep the modeller ADEQUATE; the differentiator is the CALC.
These 10 steps get us to a calc-ready mesh without over-building the solid modeller.

**Role key:** 🟢 = I code (cad_light / cad_app glue) · 🟡 = cad_solid = MD-only (I spec, don't
write Rust unless owner waives) · ⚪ = owner decision needed first.

---

## The 10 steps

**1 — Per-surface mesh identity.** 🟡🟢
Each triangle of the Factory `SolidMesh` carries which FACE/feature it came from + its normal, so
surfaces are addressable (prereq for per-surface materials). *Proof:* pick a face → its triangles
highlight. *Touches:* cad_solid mesh output (spec) + cad_app.

**2 — Seam: Factory mesh → calc scene (Phase 0).** 🟢
Convert `SolidMesh` → `cad_light::Mesh`, wire into the calc's scene with a uniform default
reflectance. *Proof:* draw a box → Calculate → the box occludes/bounces light. *This is Phase 0 of
[[project_simlux_factory_is_the_room]] — the risk-buy.* No cad_solid change.

**3 — Surface auto-classification.** 🟢
Classify each surface by normal: up = FLOOR, down = CEILING, horizontal-normal = WALL — the
DIALux-ish default. *Proof:* a room's surfaces label correctly. *Touches:* cad_app/cad_light.

**4 — Reflectance / material model.** 🟢
`Material { reflectance, color }` per surface; defaults by class (floor ≈ 0.2, wall ≈ 0.5,
ceiling ≈ 0.7), overridable. *This is the CONFIRMED substance* — the calc's per-surface input.
*Proof:* changing wall reflectance changes the bounce. *Touches:* cad_light.

**5 — Room shell (enclosure).** 🟢
2D wall plan → extrude walls (alive-wall promotion, done) → cap with FLOOR + CEILING → a CLOSED
room. *Proof:* a rectangular plan yields a 6-surface sealed box. *Touches:* cad_app.

**6 — Openings (doors / windows).** ⚪🟡
Cut openings from walls via the standalone **boolean-difference** command (select wall + opening
solid → difference). *Blocked on the cad_solid multi-body decision*
([[project_simlux_3d_factory_todo]]). *Proof:* a window hole appears; daylight passes through it.

**7 — Room objects / furniture.** 🟢
Place primitives (point-placement, done) as occluders/furniture inside the room; they join the
mesh + cast/receive light. *Proof:* a column shadows the floor. *Touches:* cad_app.

**8 — Material-assignment UI.** 🟢
Pick a surface → assign a material (reflectance) from a small library — DIALux's material catalog,
minimal. *Proof:* select the floor, set 0.3, recalc. *Touches:* cad_app + cad_light.

**9 — Watertight mesh assembly + validation.** 🟡🟢
Merge shell + openings + objects into ONE tessellated mesh + per-surface reflectance, and
**validate watertightness** (no gaps — radiosity/photon LEAK through cracks; this is the classic
failure). *Proof:* a leak-check reports "sealed"; a deliberate gap is flagged. *Touches:* cad_solid
(spec) + cad_app.

**10 — Calc-ready export → run the calc.** 🟢 **← TARGET REACHED**
Hand the assembled mesh + reflectance (+ placed luminaires / IES) to `cad_light`'s calc; produce
illuminance on the work-plane. *Proof:* a lit room with a plausible lux grid. *"Mesh creation"
is done — it feeds the calc.* *Touches:* cad_light (the differentiator — most of the future work
lives past here, in calc accuracy).

---

## Dependencies & sequencing
- **2 → 3 → 4** is the spine (mesh into calc → classify → materials). Do these first; they prove
  the whole pipeline with the geometry we already make.
- **5** can proceed in parallel (shell authoring).
- **6** waits on the boolean decision (⚪). Until then, model openings as separate solids.
- **9** is the gate to a *correct* calc — watertightness is non-negotiable for radiosity/photon.
- Beyond **10**: the real, differentiating work is calc accuracy (CIE-171-style validation), which
  is `cad_light`, not the modeller.

## We follow this plan
Steps mirrored into the session todo list and saved as memory
[[project_simlux_3d_roadmap_to_mesh]]. Run: `cd ~/workspace/simLUX/3D_Factory && cargo run -p cad_app`.
