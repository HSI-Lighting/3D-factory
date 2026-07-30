# Bundled CC0 asset library

Starter furniture models and surface textures for the 3D Factory, all released under
**Creative Commons CC0 1.0 (public domain)** — free for any use, commercial included, with
no attribution required.

Source: **Poly Haven** (https://polyhaven.com) — every asset here is CC0.

## furniture/
`*.fbx` — furniture meshes (import via ▼ Furniture → Import). `*_diffuse.jpg` is the
matching albedo map you can apply with the texture tools (FBX carries no material here).

- CoffeeTable_01, ArmChair_01, Sofa_01, WoodenChair_01, WoodenTable_01, Shelf_01,
  Ottoman_01, GreenChair_01

## furniture-gltf/
Same models as **glTF** (`<Name>/<Name>.gltf` + `.bin` + `textures/`). Import the `.gltf` and
it arrives **with its own texture already applied** (real UVs + base-colour map — the PBR
passthrough). This is the recommended format: one import, textured result, no manual step.

- CoffeeTable_01, ArmChair_01

## textures/
`*.jpg` — tileable surface textures. Select an object, then ▼ Textures →
"📂 Load texture from file…" and pick one.

- herringbone_parquet, brown_planks_03, brick_floor, brushed_concrete,
  marble_01, brown_leather, denim_fabric, floor_tiles_02

## Adding more
Grab any CC0 asset from https://polyhaven.com or https://ambientcg.com. Furniture: download
the **FBX** (1k is smallest). Textures: download the **Diffuse/Albedo** JPG. Drop them in the
matching folder here. Avoid CC-BY (needs attribution) and GPL assets for a commercial build.
