"""Build the binary-FBX texture fixtures the importer is tested against.

There was no FBX on this machine carrying an image, so the binary reader had nothing to be
validated against. This makes one deliberately: a cube whose SIDES wear a four-quadrant image
and whose TOP wears a plain red material, exported twice —

  tex_cube_embedded.fbx   textures packed INSIDE the file (Video > Content bytes)
  tex_cube_external.fbx   texture beside the file as tex_cube.png (RelativeFilename)
  tex_cube_normalmap.fbx  the same, plus a NORMAL MAP on the same material
  tex_cube.png            that texture

The normal-map variant exists because binding the wrong map as base colour is a real failure we
have already been bitten by (a curvature mask exported as the villa roof's colour, so the roof
rendered white). Two images on one material forces the reader to choose by what each one FEEDS.

Two materials on one mesh is the point: it forces the reader to honour LayerElementMaterial
(per-polygon material index) rather than assuming one material per geometry. The quadrant
colours are pure and unequal so a UV that is wrong — flipped, unindexed, or off by a vertex —
shows up as the wrong colour rather than as something that merely looks plausible.

Run:  "G:\\blender dev\\headless.cmd" "G:\\3d factory\\assets\\test\\fbx\\make_fbx_fixtures.py"
"""
import os

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
PNG = os.path.join(HERE, "tex_cube.png")

# Quadrant colours, in the order (u<.5,v<.5), (u>.5,v<.5), (u<.5,v>.5), (u>.5,v>.5).
QUADS = [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0), (1.0, 1.0, 0.0)]
RES = 64

bpy.ops.wm.read_factory_settings(use_empty=True)

# ---- the texture -----------------------------------------------------------------------
img = bpy.data.images.new("quadrants", RES, RES, alpha=True)
px = [0.0] * (RES * RES * 4)
for y in range(RES):
    for x in range(RES):
        q = (1 if x >= RES // 2 else 0) + (2 if y >= RES // 2 else 0)
        r, g, b = QUADS[q]
        i = (y * RES + x) * 4
        px[i:i + 4] = [r, g, b, 1.0]
img.pixels = px
img.filepath_raw = PNG
img.file_format = "PNG"
img.save()

# ---- materials -------------------------------------------------------------------------
tex_mat = bpy.data.materials.new("QuadTex")
tex_mat.use_nodes = True
bsdf = tex_mat.node_tree.nodes["Principled BSDF"]
node = tex_mat.node_tree.nodes.new("ShaderNodeTexImage")
node.image = img
tex_mat.node_tree.links.new(bsdf.inputs["Base Color"], node.outputs["Color"])

red_mat = bpy.data.materials.new("PlainRed")
red_mat.use_nodes = True
red_mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.8, 0.1, 0.1, 1.0)
red_mat.diffuse_color = (0.8, 0.1, 0.1, 1.0)  # what the FBX DiffuseColor property carries

# ---- the cube --------------------------------------------------------------------------
bpy.ops.mesh.primitive_cube_add(size=2.0, location=(0, 0, 0))
cube = bpy.context.active_object
cube.name = "TexCube"
cube.data.materials.append(tex_mat)   # slot 0
cube.data.materials.append(red_mat)   # slot 1

# Cube-project so every face gets the full 0..1 square — each side then shows all four quadrants.
bpy.ops.object.mode_set(mode="EDIT")
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.uv.cube_project(cube_size=2.0)
bpy.ops.object.mode_set(mode="OBJECT")

# The +Z face (one quad → 2 triangles) goes to the red slot.
top = [p for p in cube.data.polygons if p.normal.z > 0.9]
assert len(top) == 1, f"expected one +Z face, got {len(top)}"
for p in top:
    p.material_index = 1

# ---- export ----------------------------------------------------------------------------
common = dict(use_selection=False, apply_unit_scale=True, bake_space_transform=False,
              object_types={"MESH"}, use_mesh_modifiers=False, mesh_smooth_type="FACE",
              add_leaf_bones=False)

bpy.ops.export_scene.fbx(filepath=os.path.join(HERE, "tex_cube_embedded.fbx"),
                         path_mode="COPY", embed_textures=True, **common)
bpy.ops.export_scene.fbx(filepath=os.path.join(HERE, "tex_cube_external.fbx"),
                         path_mode="RELATIVE", embed_textures=False, **common)

# ---- and once more with a normal map competing for the same material ---------------------
nrm = bpy.data.images.new("flat_normal", 32, 32, alpha=True)
nrm.pixels = [0.5, 0.5, 1.0, 1.0] * (32 * 32)   # the neutral tangent-space normal
nrm.filepath_raw = os.path.join(HERE, "tex_cube_normal.png")
nrm.file_format = "PNG"
nrm.save()

nt = tex_mat.node_tree
nrm_tex = nt.nodes.new("ShaderNodeTexImage")
nrm_tex.image = nrm
nrm_tex.image.colorspace_settings.name = "Non-Color"
nrm_map = nt.nodes.new("ShaderNodeNormalMap")
nt.links.new(nrm_map.inputs["Color"], nrm_tex.outputs["Color"])
nt.links.new(bsdf.inputs["Normal"], nrm_map.outputs["Normal"])

bpy.ops.export_scene.fbx(filepath=os.path.join(HERE, "tex_cube_normalmap.fbx"),
                         path_mode="COPY", embed_textures=True, **common)

# ---- and once with ONLY the normal map, no colour image at all ---------------------------
# The reader must fall back to the material's diffuse colour here. Binding the normal map as
# base colour would paint the cube flat lilac — a picture of its bumps.
nt.links.remove(bsdf.inputs["Base Color"].links[0])
bsdf.inputs["Base Color"].default_value = (0.1, 0.35, 0.8, 1.0)   # a colour nothing else is
tex_mat.diffuse_color = (0.1, 0.35, 0.8, 1.0)
bpy.ops.export_scene.fbx(filepath=os.path.join(HERE, "tex_cube_normalonly.fbx"),
                         path_mode="COPY", embed_textures=True, **common)

for f in ("tex_cube_embedded.fbx", "tex_cube_external.fbx", "tex_cube_normalmap.fbx",
          "tex_cube_normalonly.fbx", "tex_cube.png", "tex_cube_normal.png"):
    p = os.path.join(HERE, f)
    print(f"  {f:<26} {os.path.getsize(p):>9,} bytes")
print("fixtures written to", HERE)
