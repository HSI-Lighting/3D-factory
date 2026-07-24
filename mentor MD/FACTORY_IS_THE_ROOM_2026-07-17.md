# Decision: 3D Factory IS the room — SIMLUX lights it

**Owner ruling, 2026-07-17:** *"this 2 basically are one thing going to do one job — 3d
factory will make 3d room ready for light calculation."* Chosen convergence: **Factory is
THE room (phased)** — the light calc reads the Factory model's mesh; the 2D-extrude path
keeps working until Factory can replace it, then the 2D plan becomes an *input* that extrudes
into a Factory solid. One model long-term, two toolsets on it: **build** (Factory) and
**calculate** (SIMLUX).

---

## Where they are today (two models, zero connection)

| | room source | state | mesh type |
|---|---|---|---|
| **SIMLUX** | 2D plan `extrude(doc, room_height)` | `self.light` — `meshes` + `materials` + `luminaires` | `cad_light::Mesh` |
| **3D Factory** | CSG solids drawn in Draw3D | `self.factory.model` → `cached: SolidMesh` | `cad_solid::SolidMesh` |

The Factory room feeds **nothing** to the light calc. That's the whole gap.

## The seam is clean

Both sides are triangle meshes. Factory already emits `SolidMesh` (`positions`+`normals`,
`cad_solid/src/lib.rs`); the light engine already consumes `Mesh`. So Factory→lighting is a
**mesh convert + hand-off** — cad_light + cad_app glue, **no cad_solid surgery** (I consume
the mesh it already produces; mentor-only rule on cad_solid respected).

## The real work is NOT the plumbing — it's materials

The light bounce needs **per-surface reflectance** (ceiling ~0.7, wall ~0.5, floor ~0.2).
The extruded-2D room gets it from layer materials; **Factory solids carry none.** So the
substance of this convergence is *"how does a face of a Factory box know it's a wall at
ρ=0.5."* Leading idea: **auto-classify by face normal** (up→ceiling, down→floor,
horizontal→wall) as the DIALux-ish default, overridable per solid/face later.

---

## Phasing — nothing breaks until it's ready to

- [ ] **Phase 0 — prove the seam (additive, mine).** Convert `factory.cached` → `cad_light::Mesh`
      and add it to the light scene *alongside* the extruded-2D meshes, with a **uniform default
      reflectance**. Draw a box → Calculate → the box participates in the bounce. Nothing
      retired. This is the whole risk-buy: if the calc respects Factory geometry, the pipeline
      is real.
- [ ] **Phase 1 — materials.** Per-surface reflectance on Factory solids; auto-classify by
      normal, with an override. This is where the DIALux fidelity actually lives.
- [ ] **Phase 2 — 2D extrude feeds Factory.** The "Move to SIMLUX layer + extrude" path
      produces a **Factory solid**, not a parallel `light.meshes` room. Now there is ONE model.
- [ ] **Phase 3 — one viewport.** The separate SIMLUX-3D render and the Factory viewport
      collapse into a single 3D view of the one room, with the light overlay on top.
- [ ] **Phase 4 — retire `light.meshes` as an independent room.** It becomes *derived* from
      the Factory model; the extrude-2D-as-its-own-room representation goes away.

**Order matters:** Phase 0 is provable and reversible. Each later phase only starts once the
previous is confirmed by the owner — same discipline as the modifier-dispatch work.

## Role boundaries on this track
- **cad_light + cad_app glue = I code** (`feedback_simlux_supervisor_only`).
- **cad_solid = MD only** (`feedback_simlux_3d_mentor_role`) — untouched; I consume `SolidMesh`.
- No commit/push unless asked.

## Run
```bash
cd ~/workspace/simLUX/3D_Factory && cargo run -p cad_app
```
