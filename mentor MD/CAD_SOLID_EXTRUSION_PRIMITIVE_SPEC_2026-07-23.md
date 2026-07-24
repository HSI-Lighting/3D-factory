# `cad_solid` — extrusion primitive (profile → solid)

**Date:** 2026-07-23 · **Status:** SPEC — awaiting owner sign-off (Rule 2) · **No Rust written**
**Blocks:** `BuildingTool::Outline` (the greyed row in 3D Factory ▸ Building)

---

## 1. Why

The Building section ships with three element tools working on existing primitives:

| Tool | Primitive | Status |
|---|---|---|
| Rectangular | `Primitive::Box { w, d, h }` | ships |
| Circular | `Primitive::Cylinder { r, h, sides }` | ships |
| Polygon (regular n-gon) | `Primitive::Frustum { r_top == r_bottom, sides, h }` | ships |
| **Building outline (arbitrary traced edge)** | **none** | **this spec** |

Every variant of `Primitive` today is an **analytic** shape — it is described by scalars and
generates its own mesh. There is no way to say *"take these points and raise them"*, so a user
cannot trace the edges of a real building. That is the gap.

## 2. The capability is already in the dependency

`cad_solid/Cargo.toml` pins csgrs at rev `5e7a37a` with the **`earcut`** feature already
enabled (the comment there notes csgrs's `sketch` module compiles unconditionally, so the
triangulator had to be on). That module provides exactly the two calls needed:

- `csgrs::sketch::Sketch::polygon(points: &[[Real; 2]], metadata) -> Sketch` — `shapes.rs:141`
- `Sketch::extrude(height: Real) -> Mesh` — `extrudes.rs:18`
  (`extrude_vector(dir)` also exists, if a sheared/leaning extrusion is ever wanted)

So this is **not** a new dependency, a new feature flag, or new geometry code. It is a new
`Primitive` variant plus one arm in `csg.rs`. That is the whole change.

## 3. The one real design problem — `Primitive` is `Copy`

```rust
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]   // lib.rs:149
pub enum Primitive { … }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]   // lib.rs:256
pub struct Feature { pub id, pub op, pub plane, pub placement, pub primitive }
```

A profile is a `Vec<Vec2>`. Putting it **inside** the enum drops `Copy` from `Primitive` **and
therefore from `Feature`**, which is passed by value throughout (`with_world_origin`,
`translated`, `rotated`, `scaled`, `mirrored` all return `Feature`). `Primitive` is mentioned
186 times across `cad_app` + `cad_solid`. This is precisely the trap that already bit
`WallInst`, which had to drop `Copy` the moment it gained `footprint` (handoff gotcha 7).

### Recommended: keep `Copy`, store profiles in a side table

```rust
pub type ProfileId = u32;

pub enum Primitive {
    …
    /// Vertical extrusion of a closed profile held in `Model::profiles`.
    Extrusion { profile: ProfileId, h: f32 },
}

pub struct Model {
    pub features: Vec<Feature>,
    pub profiles: Vec<Profile>,        // NEW — id-keyed, never index-keyed
    #[serde(skip)] pub sketches: Vec<Sketch>,
}

pub struct Profile { pub id: ProfileId, pub pts: Vec<Vec2> }
```

- **`Primitive` and `Feature` stay `Copy`.** Zero churn across the other 186 sites.
- It matches the idiom the codebase already uses: *"feature ids are stable keys, not indices"*
  (`Model::remove(id)` does not renumber). `ProfileId` behaves the same way.
- Serde round-trips unchanged — `Model` already derives it and `profiles` is a plain `Vec`.
- Profiles are naturally **shared**: every storey of the same outline references one profile,
  so editing the outline once re-derives every storey. That falls out for free, and is the
  behaviour a building modeller wants.

### Alternatives considered

| Option | Verdict |
|---|---|
| `Extrusion { profile: Vec<Vec2>, h }` inline | Simplest to write, but drops `Copy` on `Primitive` **and** `Feature` — wide, mechanical, risky churn for no gain |
| `Arc<[Vec2]>` inline | Still not `Copy`; adds shared-mutation questions; `serde` needs help |
| Compose from Boxes (app-side, no `cad_solid` change) | Rejected — an N-gon of boxes is not a solid, has no correct top/bottom face, and would hand the light calc a non-watertight mesh (Track B) |

## 4. Scope

**In:**
1. `Primitive::Extrusion { profile: ProfileId, h: f32 }`.
2. `Model::profiles` + `add_profile(pts) -> ProfileId` / `profile(id) -> Option<&Profile>`.
3. One arm in `csg.rs::primitive_mesh` → `Sketch::polygon(pts).extrude(h)`.
4. `local_aabb()` for the variant — the profile's 2D bounds × `[0, h]`. Needed by
   `world_aabb`, which drives ray-pick selection; a wrong AABB makes the solid unpickable.

**Out (deliberately):**
- Holes / inner rings. One closed outer loop only. Openings are the boolean decision
  (`BOOLEAN_AS_COMMAND_2026-07-17.md`), not this.
- Tapered or leaning extrusion. That is the `Feature` tilt-DOF question (same blocker as
  `rake_deg`), specced separately.
- Any UI. The app-side journey is §6.

## 5. Validation

| Rule | Behaviour |
|---|---|
| Fewer than 3 points | rejected at `add_profile`, no profile minted |
| `h <= 0` | rejected — a zero-height extrusion is not a solid |
| Unclosed point list | closed implicitly (last→first); the stored list never duplicates the first point |
| Self-intersecting profile | **reject with a message.** earcut's output for a self-intersecting loop is undefined, and a non-watertight mesh would silently poison the light calc downstream (Track B) |
| Winding | normalised to CCW on insert, so extruded normals point consistently outward |
| Dangling `ProfileId` | `primitive_mesh` returns an empty mesh rather than panicking — a stale id must never take the app down |

Tests to land with it: round-trip a square profile → 12 triangles, non-zero volume;
`local_aabb` matches the profile bounds × height; each rejection rule asserted; serde
round-trip of a `Model` carrying a profile.

## 6. The app-side journey (separate slice, for confirmation under Rule 8)

Proposed, to match the confirmed wall journey (*draft in 2D → select → right-click → Make 3D
wall*) rather than inventing a new gesture:

> Draft a **closed** polyline with the real 2D tools → select it → right-click → **Make
> building** → it rises to `FactoryState::building_height`.

This introduces **no new command verb** (Rule 4) and reuses the whole 2D toolset for the
tracing (Rule 3). The Building section's greyed *Building outline* row becomes a live entry
point to the same flow.

## 7. Decision requested

1. Approve `Primitive::Extrusion` with the **`ProfileId` side-table** shape (§3), preserving
   `Copy`?
2. Confirm the right-click journey in §6, or name a different gesture?

Nothing is written into `cad_solid` until (1) is answered.
