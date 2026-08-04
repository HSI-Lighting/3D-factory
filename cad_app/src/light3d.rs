//! SIMLUX 3D viewport — a small OpenGL renderer for the extruded room.
//!
//! It renders the `cad_light` meshes (flat-shaded, depth-tested) into an
//! **offscreen FBO** (colour texture + depth renderbuffer), then composites that
//! texture into the egui panel's rect with a full-rect quad. Going through an FBO
//! means we don't depend on the eframe window having a depth buffer, and the 3D
//! pass never disturbs egui's own framebuffer state.
//!
//! Driven from inside an egui `PaintCallback` (GL thread), mirroring `gpu.rs`.

use std::mem::size_of;

use eframe::glow;
use eframe::glow::HasContext;
use glam::{Mat4, Vec3};

use cad_light::{LuxGrid, Material, Mesh};

/// One 3D vertex: position (metres, Z-up) + baked RGB colour + the AMBIENT share of that colour.
///
/// The colour is **scene-referred linear light** — the CPU shader in `factory.rs` decodes the
/// material's authored sRGB and multiplies it by linear irradiance, so nothing is re-encoded on the
/// way to the framebuffer. `amb` is what fraction of that light arrived as ambient (sky or fill)
/// rather than as the key/sun, which is the only part screen-space occlusion may darken; without it
/// AO would grey out a crease standing in full sunlight.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// The surface's OWN colour, in authored sRGB — not a lit one. The fragment shader lights it,
    /// which is what lets the sun move without rebuilding this buffer.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// World normal. Zero marks a UI swatch, which is passed through unlit.
    pub nx: f32,
    pub ny: f32,
    pub nz: f32,
    /// Which lighting response to apply — `SHADE_UI` / `SHADE_SCENE` / `SHADE_FURNITURE` from
    /// `factory.rs`. Per vertex, because one buffer mixes walls, furniture and overlays.
    pub mode: f32,
}

/// One TRANSLUCENT vertex: position + baked RGB + per-vertex OPACITY. Used only by the blended
/// transparent pass (glass panes and other see-through furniture faces), so the opaque `V3`
/// fast path keeps its 24-byte, alpha-free layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct V3A {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// One TEXTURED vertex: position (metres, Z-up) + UV + a baked scalar shade (0..1) + opacity.
/// The textured pass samples the bound image at `u,v` and multiplies by `s`, so a surface keeps
/// its flat-shaded lighting while showing the pasted picture. `a` is the vertex opacity — 1.0 for
/// every opaque surface (feature walls/floors, ordinary furniture); below 1.0 only for textured
/// glass, which is drawn in the blended textured pass.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TexVtx {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub u: f32,
    pub v: f32,
    pub s: f32,
    pub a: f32,
}

/// A PROCEDURAL material evaluated in the fragment shader from world position — the engine's
/// answer to Blender's noise→ColorRamp materials (wood grain, marble, …). `mode` 0 = "not
/// procedural, sample the bound image" (the default); 1 = wood, 2 = marble, 3 = noise, 4 = checker.
/// `scale` is ANISOTROPIC (squash across the grain, stretch along it) and read against WORLD
/// position, so the pattern runs continuously across every piece — the grain-match Blender gets for
/// free from object coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ProcParams {
    pub mode: i32,
    pub col_a: [f32; 3],
    pub col_b: [f32; 3],
    pub scale: [f32; 3],
    pub detail: f32,
    pub rough: f32,
    pub contrast: f32,
    pub ramp: [f32; 2],
    /// Surface roughness at the two ends of the SAME pattern field that drives the colour — dark
    /// grain rougher than light, mortar rougher than tile. Equal values mean a uniform finish.
    /// This is most of what separates a procedural that reads as a material from one that reads as
    /// a picture painted on plastic: a real surface varies in gloss wherever it varies in colour.
    pub rough_lo: f32,
    pub rough_hi: f32,
    /// Bump strength. The pattern field is treated as a height field and its gradient perturbs the
    /// shading normal, so the grain catches light instead of just tinting it. 0 = flat.
    pub bump: f32,
}

impl Default for ProcParams {
    fn default() -> Self {
        Self {
            mode: 0,
            col_a: [0.5, 0.5, 0.5],
            col_b: [0.5, 0.5, 0.5],
            scale: [1.0, 1.0, 1.0],
            detail: 6.0,
            rough: 0.6,
            contrast: 1.0,
            ramp: [0.35, 0.65],
            rough_lo: 0.5,
            rough_hi: 0.5,
            bump: 0.0,
        }
    }
}

const CEILING: u32 = 2; // material id skipped in the viewer so we can look in
const FLOOR: u32 = 0;

const IDENTITY16: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Per-texture PBR maps (Texture Phase 2): tangent-space **normal** + **roughness** map indices (into
/// the app's texture list, so they upload through the normal texture cache), plus the scalar
/// Principled parameters used when there's no map. `metallic`/`ior` reach the raster shader from
/// here — before Phase 1 they existed only in the path tracer, so a "metal" in the Materials Factory
/// rendered as grey plastic in the viewport.
#[derive(Clone, Copy, Debug)]
pub struct PbrParams {
    pub normal_idx: Option<usize>,
    pub rough_idx: Option<usize>,
    /// Metallic and ambient-occlusion maps — the other two members of a downloaded PBR texture set.
    pub metal_idx: Option<usize>,
    pub ao_idx: Option<usize>,
    /// Map this material in WORLD space (triplanar) at `tiles_per_m` rather than through the mesh's
    /// own UVs. The default for architecture, which is extruded from a plan and has none.
    pub triplanar: bool,
    pub tiles_per_m: f32,
    pub roughness: f32,
    pub metallic: f32,
    /// Dielectric index of refraction — sets the specular F0 (1.5 ⇒ the usual 0.04).
    pub ior: f32,
    /// Emitted radiance, already multiplied by its strength and already linear. An emissive
    /// material used to glow only in the path tracer; it now lights up the viewport as well.
    pub emission: [f32; 3],
    /// CLEARCOAT: a thin varnish over the material, with its own smooth specular lobe. 0 = bare.
    /// Reflects about the GEOMETRIC normal — lacquer fills the grain, so a polished tabletop shows
    /// the timber but mirrors the window as one clean shape.
    pub clearcoat: f32,
    pub clearcoat_rough: f32,
    /// SHEEN: the pale rim fabric gets at grazing angles from light scattering through the fuzz
    /// standing off its surface. 0 = none. Without it velvet, felt and heavy curtains all render
    /// as matte plastic, because a diffuse lobe has nothing that brightens along the surface.
    pub sheen: f32,
    /// The colour of that fuzz — usually near-white even on a dark fabric, which is most of why
    /// black velvet reads as velvet.
    pub sheen_tint: [f32; 3],
}

impl Default for PbrParams {
    fn default() -> Self {
        Self { normal_idx: None, rough_idx: None, metal_idx: None, ao_idx: None, triplanar: false, tiles_per_m: 1.0, roughness: 0.5, metallic: 0.0, ior: 1.5, emission: [0.0; 3], clearcoat: 0.0, clearcoat_rough: 0.1, sheen: 0.0, sheen_tint: [1.0; 3] }
    }
}

/// Fixed key light for flat shading (points down-ish onto the scene).
fn light_dir() -> Vec3 {
    Vec3::new(0.35, 0.25, 0.9).normalize()
}

fn shade(base: [f32; 3], n: Vec3) -> [f32; 3] {
    let k = 0.35 + 0.65 * n.dot(light_dir()).abs();
    [base[0] * k, base[1] * k, base[2] * k]
}

fn material_color(materials: &[Material], id: u32) -> [f32; 3] {
    materials.iter().find(|m| m.id == id).map(|m| m.color).unwrap_or([0.7, 0.7, 0.7])
}

/// Build the flat-shaded triangle soup for the room. The ceiling is skipped so
/// the camera can look down into the room. If `floor_grid` is given, floor
/// vertices are coloured by sampled lux (P3) instead of the floor material.
pub fn build_scene_verts(
    meshes: &[Mesh],
    materials: &[Material],
    floor_grid: Option<(&LuxGrid, &cad_light::CalcPlane, f64, fn(f32) -> (f32, f32, f32))>,
) -> Vec<V3> {
    let mut out = Vec::new();
    for m in meshes {
        if m.material == CEILING {
            continue;
        }
        let base = material_color(materials, m.material);
        for t in &m.triangles {
            let (Some(a), Some(b), Some(c)) =
                (m.vertices.get(t.a as usize), m.vertices.get(t.b as usize), m.vertices.get(t.c as usize))
            else {
                continue;
            };
            let (pa, pb, pc) = (a.to_vec3(), b.to_vec3(), c.to_vec3());
            let n = (pb - pa).cross(pc - pa).normalize_or_zero();
            let flat = shade(base, n);
            for p in [pa, pb, pc] {
                // Floor + a lux grid → colour by illuminance; else flat shade.
                let col = match &floor_grid {
                    Some((grid, plane, maxv, cmap)) if m.material == FLOOR => {
                        let lux = sample_lux(grid, plane, p);
                        let (r, g, b) = cmap((lux / *maxv) as f32);
                        [r, g, b]
                    }
                    _ => flat,
                };
                // amb = 0: this is the SIMLUX lighting view, whose colours are a false-colour lux
                // scale or a fixed studio shade — neither is something occlusion may re-grade.
                out.push(V3 { x: p.x, y: p.y, z: p.z, r: col[0], g: col[1], b: col[2], nx: 0.0, ny: 0.0, nz: 0.0, mode: 0.0 });
            }
        }
    }
    out
}

/// Append a small bright octahedron marking a luminaire at (x, y, z).
pub fn push_luminaire_marker(out: &mut Vec<V3>, x: f32, y: f32, z: f32, s: f32) {
    let c = [1.0, 0.86, 0.38];
    let v = |dx: f32, dy: f32, dz: f32| V3 { x: x + dx, y: y + dy, z: z + dz, r: c[0], g: c[1], b: c[2], nx: 0.0, ny: 0.0, nz: 0.0, mode: 0.0 };
    let top = v(0.0, 0.0, s);
    let bot = v(0.0, 0.0, -s);
    let pn = v(s, 0.0, 0.0);
    let pe = v(0.0, s, 0.0);
    let ps = v(-s, 0.0, 0.0);
    let pw = v(0.0, -s, 0.0);
    let mut tri = |a: V3, b: V3, cc: V3| {
        out.push(a);
        out.push(b);
        out.push(cc);
    };
    tri(top, pn, pe);
    tri(top, pe, ps);
    tri(top, ps, pw);
    tri(top, pw, pn);
    tri(bot, pe, pn);
    tri(bot, ps, pe);
    tri(bot, pw, ps);
    tri(bot, pn, pw);
}

/// Nearest-cell lux at a floor point (used for the P3 3D heatmap).
fn sample_lux(grid: &LuxGrid, plane: &cad_light::CalcPlane, p: Vec3) -> f64 {
    if grid.values.is_empty() {
        return 0.0;
    }
    let dx = plane.width / plane.cols.max(1) as f32;
    let dy = plane.depth / plane.rows.max(1) as f32;
    let col = (((p.x - plane.origin.x) / dx) as i32).clamp(0, plane.cols as i32 - 1) as u32;
    let row = (((p.y - plane.origin.y) / dy) as i32).clamp(0, plane.rows as i32 - 1) as u32;
    grid.values[(row * plane.cols + col) as usize]
}

/// The orbit camera's world EYE position for `(yaw, pitch, dist, target)` — same formula [`mvp`]
/// uses, exposed so the caller can pass it to `render` for the reflection sheen.
pub fn cam_eye(yaw: f32, pitch: f32, dist: f32, target: [f32; 3]) -> [f32; 3] {
    let t = Vec3::from(target);
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let (cy, sy) = (yaw.cos(), yaw.sin());
    (t + Vec3::new(cp * cy, cp * sy, sp) * dist.max(0.1)).to_array()
}

/// Orbit-camera MVP: yaw/pitch around `target`, `dist` away, GL depth convention.
pub fn mvp(yaw: f32, pitch: f32, dist: f32, target: [f32; 3], aspect: f32, ortho: bool) -> [f32; 16] {
    let t = Vec3::from(target);
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let (cy, sy) = (yaw.cos(), yaw.sin());
    let eye = t + Vec3::new(cp * cy, cp * sy, sp) * dist.max(0.1);
    // Up = Z, but flip to Y when looking (near-)straight down/up so the Top/Bottom nav
    // views don't hit the look_at degeneracy (view dir ∥ up → NaN matrix).
    let up = if sp.abs() > 0.999 { Vec3::Y } else { Vec3::Z };
    let view = Mat4::look_at_rh(eye, t, up);
    let proj = if ortho {
        // PARALLEL projection — a true CAD Top/Front/… view (a cylinder is a perfect
        // circle in Top, no perspective barrel). Framed to match the perspective's
        // apparent size at the target so switching modes doesn't jump the zoom.
        let hh = dist.max(0.1) * (45f32.to_radians() * 0.5).tan();
        let hw = hh * aspect.max(0.01);
        let z = (dist * 20.0).max(200.0);
        Mat4::orthographic_rh_gl(-hw, hw, -hh, hh, -z, z)
    } else {
        // Near scales with distance so the depth buffer keeps precision when dollied far
        // back over a large plan (fixed 0.05 vs a 600k far plane = severe z-fighting). The
        // 0.05 floor preserves close-up behaviour exactly for normal-sized models.
        let near = (dist * 0.001).max(0.05);
        Mat4::perspective_rh_gl(45f32.to_radians(), aspect.max(0.01), near, (dist * 6.0).max(80.0))
    };
    (proj * view).to_cols_array()
}

// Depth-only pass from the sun's point of view — the shadow map. Positions only; the fragment
// stage writes nothing but depth.
const DEPTH_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    uniform mat4 u_depth_mvp;
    void main() { gl_Position = u_depth_mvp * vec4(a_pos, 1.0); }
"#;
const DEPTH_FS: &str = r#"
    #version 330 core
    void main() {}
"#;

// A 3×3 PCF shadow lookup shared by the scene + textured fragment shaders. Returns 1 = fully lit,
// 0 = fully shadowed. Off-map or beyond the far plane counts as lit.
// CASCADED SHADOW MAPS.
//
// One shadow map stretched over a whole site spends nearly all of its resolution on ground nobody
// is looking at. Fitted to a 100 m villa plot, a 2048² map is 7 cm per texel — a window mullion's
// shadow is two texels wide and comes out as mush, while the same map wastes half its area on the
// far end of the garden. Cascades give the near ground its own tight map and let the distant
// ground have the coarse one, which is where the resolution actually belongs.
//
// Selection is by CONTAINMENT, not by a split distance: walk the cascades tightest-first and use
// the first one the fragment actually falls inside. That needs no split-plane uniforms, and — more
// to the point — it cannot select a cascade that does not cover the fragment, which is the classic
// way a distance-based selector puts a hard black line across the floor at a cascade boundary.
const CASCADE_MAX: usize = 3;
const SHADOW_GLSL: &str = r#"
    uniform int u_shadow_on;
    uniform int u_csm_n;                  // cascades actually in use, 1..CASCADE_MAX
    uniform mat4 u_light_mvp[CASCADE_MAX_GLSL];
    uniform sampler2DArray u_shadow;
    float shadow_lit(vec3 wpos) {
        for (int c = 0; c < CASCADE_MAX_GLSL; c++) {
            if (c >= u_csm_n) break;
            vec4 lc = u_light_mvp[c] * vec4(wpos, 1.0);
            vec3 p = lc.xyz / lc.w * 0.5 + 0.5;
            // Not inside this cascade — try the next, coarser one.
            if (p.z > 1.0 || p.x < 0.0 || p.x > 1.0 || p.y < 0.0 || p.y > 1.0) continue;
            // One constant bias serves every cascade: the ortho depth range and the texel's world
            // size both scale with the cascade radius, so the NDC error a slope produces across one
            // texel is the same in all of them.
            float bias = 0.0022;
            vec2 tx = 1.0 / vec2(textureSize(u_shadow, 0).xy);
            float lit = 0.0;
            for (int y = -1; y <= 1; y++)
                for (int x = -1; x <= 1; x++) {
                    float d = texture(u_shadow, vec3(p.xy + vec2(x, y) * tx, float(c))).r;
                    lit += (p.z - bias > d) ? 0.0 : 1.0;
                }
            return lit / 9.0;
        }
        return 1.0;   // beyond every cascade — unshadowed rather than wrongly dark
    }
"#;

/// [`SHADOW_GLSL`] with the cascade count substituted in — GLSL needs a compile-time array size,
/// and Rust owns the number.
fn shadow_glsl() -> String {
    SHADOW_GLSL.replace("CASCADE_MAX_GLSL", &CASCADE_MAX.to_string())
}

/// Side of one cascade's shadow map, in texels. Public so the app can snap its cascade boxes to
/// this grid — snapping to the wrong size is the same as not snapping at all.
pub const SHADOW_MAP_SIZE: i32 = 2048;

/// The most cascades the shader can sample. Public so the UI can bound its own control.
pub const MAX_CASCADES: usize = CASCADE_MAX;

const SCENE_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec3 a_col;
    layout(location=2) in vec3 a_nrm;
    layout(location=3) in float a_mode;
    uniform mat4 u_mvp;
    uniform mat4 u_model;   // world = u_model * a_pos (identity for the world-space scene batch)
    out vec3 v_col;
    out vec3 v_wpos;
    out vec3 v_nrm;
    flat out float v_mode;
    void main() {
        gl_Position = u_mvp * vec4(a_pos, 1.0);
        v_col = a_col;
        // The normal rides the model matrix like the position. A drag uses a rigid model matrix
        // (rotation + uniform scale), so the inverse-transpose is not needed here.
        v_nrm = mat3(u_model) * a_nrm;
        v_mode = a_mode;
        v_wpos = (u_model * vec4(a_pos, 1.0)).xyz;
    }
"#;

const SCENE_FS: &str = r#"
    #version 330 core
    in vec3 v_col;
    in vec3 v_wpos;
    in vec3 v_nrm;
    flat in float v_mode;
    // TWO targets. 0 = light that is already accounted for (direct sun, specular, emission);
    // 1 = the AMBIENT share, which is the only part screen-space occlusion is entitled to darken.
    // Splitting here rather than multiplying AO into the final image is what stops a crease in
    // full sunlight from going grey.
    layout(location=0) out vec4 frag;
    layout(location=1) out vec4 amb_out;
    ALBEDO_OUT_GLSL
    // Per-PASS alpha: V3 carries no alpha channel, so translucency for the overlay
    // pass (selection shade / modifier ghosts) rides on this uniform. 1.0 for the
    // opaque solid + line passes.
    uniform float u_alpha;
    // 1 = the vertex colours are authored sRGB and must be decoded before they mix with light —
    // the overlay/line passes, whose colours are UI swatches. 0 = they already carry scene-referred
    // linear light (the CPU-shaded solids) or display values (the SIMLUX lux heatmap).
    uniform int u_linearize;
    SHADOW_GLSL
    SRGB_GLSL
    SKY_GLSL
    const vec3 STUDIO_DIR = vec3(0.35, 0.25, 0.9);
    void main() {
        // A UI swatch — a grid line, a selection tint, a lux heatmap value. Not a surface, so it
        // is not lit, and ambient occlusion has no business dimming it either.
        if (v_mode < 0.5) {
            vec3 col = u_linearize == 1 ? srgb_to_lin(v_col) : v_col;
            frag = vec4(col, u_alpha);
            amb_out = vec4(0.0, 0.0, 0.0, u_alpha);
            // A UI swatch is not a surface, so it has no albedo to bounce with. Zero, so screen
            // -space GI treats a grid line as what it is rather than as a glowing white wire.
            alb_out = vec4(0.0);
            return;
        }
        // A LIT SURFACE. The albedo and the normal arrive per vertex and the light is applied
        // here, per fragment — the twin of `shade` / `shade_furniture` in factory.rs, which the
        // tests compare against. Doing it here is what lets the sun move, a shadow soften or a
        // sample jitter without rebuilding a single vertex.
        vec3 albedo = srgb_to_lin(v_col);
        // TWO-SIDED. Foliage, fabric and any imported thin card is one sheet of triangles whose
        // authored normal points one way; seen from behind it takes zero direct sun and only the
        // dim downward ambient, and renders BLACK — which is exactly how the villa's trees came
        // out. The textured path already faces its normal to the viewer; this is the flat path
        // catching up. The path tracer flips the same way (`if tri.n.dot(rd) > 0 { -tri.n }`), so
        // this also stops the viewport and an offline render disagreeing about a leaf.
        vec3 N = normalize(v_nrm);
        if (!gl_FrontFacing) N = -N;
        bool furniture = v_mode > 1.5;
        vec3 ambient, direct;
        if (u_sky_on == 1) {
            // Furniture keeps a slightly higher ambient floor so an imported mesh never reads
            // murky — the 0.05 in `shade_furniture`.
            float extra = furniture ? 0.05 : 0.0;
            ambient = albedo * (sh_ambient(N) + vec3(extra));
            float lit = max(dot(N, u_sky_sun), 0.0);
            float sh = (u_shadow_on == 1) ? shadow_lit(v_wpos) : 1.0;
            direct = albedo * u_sky_sun_col * (lit * sh);
        } else {
            // Studio: a constant fill plus a two-sided directional term, exactly as the CPU built
            // it. Furniture uses 0.6 / 0.4 where the scene uses 0.35 / 0.65.
            float fill = furniture ? 0.6 : 0.35;
            float k = (furniture ? 0.4 : 0.65) * abs(dot(N, normalize(STUDIO_DIR)));
            ambient = albedo * fill;
            direct = albedo * k;
        }
        frag = vec4(direct, u_alpha);
        amb_out = vec4(ambient, u_alpha);
        alb_out = vec4(albedo, u_alpha);
    }
"#;

// Transparent furniture pass: pos + colour + per-vertex opacity. Same flat colour as the solid
// pass, but alpha rides on the vertex so glass panes blend. Drawn AFTER all opaque geometry,
// depth-tested but with depth writes OFF, back-to-front (sorted CPU-side).
const TRANSP_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec3 a_col;
    layout(location=2) in float a_a;
    uniform mat4 u_mvp;
    out vec3 v_col;
    out float v_a;
    void main() { gl_Position = u_mvp * vec4(a_pos, 1.0); v_col = a_col; v_a = a_a; }
"#;

const TRANSP_FS: &str = r#"
    #version 330 core
    in vec3 v_col;
    in float v_a;
    layout(location=0) out vec4 frag;
    layout(location=1) out vec4 amb_out;
    ALBEDO_OUT_GLSL
    uniform int u_linearize;
    SRGB_GLSL
    void main() {
        frag = vec4(u_linearize == 1 ? srgb_to_lin(v_col) : v_col, v_a);
        // Glass contributes no occludable ambient of its own, but it must still blend against the
        // ambient buffer — writing (0,0,0,alpha) attenuates what is behind it by the same factor
        // the colour buffer uses, so a pane does not leave an un-dimmed ghost in the AO term.
        amb_out = vec4(0.0, 0.0, 0.0, v_a);
        // Glass does not bounce diffuse light — it transmits. Leaving the albedo buffer at what
        // is BEHIND the pane is also what we want for screen-space GI: the bounce should come from
        // the wall you can see through the window, not from the window.
        alb_out = vec4(0.0, 0.0, 0.0, v_a);
    }
"#;

// Textured surface pass: pos + uv + baked shade. Samples the bound image and keeps the
// flat-shaded lighting via the scalar `a_shade`. Used for objects with a pasted texture.
const TEX_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec2 a_uv;
    layout(location=2) in float a_shade;
    layout(location=3) in float a_a;
    uniform mat4 u_mvp;
    uniform mat4 u_model;   // world = u_model * a_pos (identity for world-space feature surfaces)
    out vec2 v_uv;
    out float v_shade;
    out float v_a;
    out vec3 v_wpos;   // WORLD position — reflection sheen, sun lighting, shadow lookup, normal maps
    void main() { gl_Position = u_mvp * vec4(a_pos, 1.0); v_uv = a_uv; v_shade = a_shade; v_a = a_a; v_wpos = (u_model * vec4(a_pos, 1.0)).xyz; }
"#;

const TEX_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    in float v_shade;
    in float v_a;
    in vec3 v_wpos;
    // 0 = direct + specular + emission · 1 = the diffuse AMBIENT term, the only thing AO darkens.
    layout(location=0) out vec4 frag;
    layout(location=1) out vec4 amb_out;
    ALBEDO_OUT_GLSL
    uniform sampler2D u_img;
    uniform vec3 u_cam;       // camera world position
    uniform float u_reflect;  // 0 = matte, 1 = full environment reflection
    // PROCEDURAL material (evaluated from world position when u_proc > 0).
    uniform int   u_proc;       // 0=image, 1=wood, 2=marble, 3=noise, 4=checker
    uniform vec3  u_col_a;      // ramp low / colour A
    uniform vec3  u_col_b;      // ramp high / colour B
    uniform vec3  u_pscale;    // anisotropic world scale (across, along, through the grain)
    uniform float u_detail;    // fbm octaves
    uniform float u_prough;    // fbm amplitude falloff
    uniform float u_pcontrast; // ramp hardness about the midpoint
    uniform vec2  u_ramp;      // ramp low/high positions
    uniform float u_rough_lo;  // surface roughness at the ramp's low end …
    uniform float u_rough_hi;  // … and at its high end (equal = uniform finish)
    uniform float u_bump;      // relief from the same field, 0 = flat
    // WORLD-SPACE (triplanar) mapping and its texel density, for geometry with no useful UVs.
    uniform int   u_triplanar;
    uniform float u_tpm;       // tiles per metre
    // Daylight (sun) — when u_sun_on, light this surface by the real sun instead of the baked shade.
    uniform int   u_sun_on;
    uniform vec3  u_sun_dir;   // TO the sun
    uniform vec3  u_sun_col;   // direct term, calibrated as irradiance/PI
    // EMISSION — an emissive material now actually glows in the viewport. Already multiplied by its
    // strength on the CPU, and already linear.
    uniform vec3  u_emission;
    // PBR maps (Texture Phase 2). Tangent-space normal + roughness, sampled per fragment; the TBN is
    // reconstructed from screen-space derivatives so NO per-vertex tangents are needed.
    uniform sampler2D u_nrm;
    uniform sampler2D u_rough;
    uniform sampler2D u_metal;
    uniform sampler2D u_aomap;
    uniform int   u_has_nrm;
    uniform int   u_has_rough;
    uniform int   u_has_metal;
    uniform int   u_has_ao;
    uniform float u_rough_base; // scalar roughness when there's no map
    uniform float u_metallic;   // 0 = dielectric, 1 = conductor (F0 becomes the albedo)
    uniform float u_ior;        // dielectric index of refraction → specular F0
    // CLEARCOAT — a thin varnish over the material. Its own smooth specular lobe on top of
    // whatever the base is doing, and it does NOT follow the base's bump: lacquer fills the grain,
    // which is precisely why a polished tabletop shows the timber underneath but reflects the
    // window as one clean shape rather than as a rippled one.
    uniform float u_coat;       // 0 = bare
    uniform float u_coat_rough;
    // SHEEN — the pale rim fabric gets at grazing angles, from light scattering through the fuzz
    // standing off its surface. Without it velvet, felt and heavy curtains all render as matte
    // plastic, because a diffuse lobe has nothing that brightens as you look along the surface.
    uniform float u_sheen;
    uniform vec3  u_sheen_tint;
    uniform int   u_clay;       // 1 = override albedo to flat clay grey (glass keeps its alpha)
    uniform int   u_hl;         // 1 = this draw's material is selected in the Materials Factory
    uniform float u_hl_k;       // highlight pulse strength 0..1 (app-driven)
    SHADOW_GLSL
    SRGB_GLSL
    SKY_GLSL
    ENV_BRDF_GLSL

    // Sample a map either with the mesh's UVs, or — in triplanar mode — as three world-axis
    // projections blended by the surface normal, which is what removes the stretching a single
    // planar projection leaves on every face that is not square to it.
    vec4 tri_or_uv(sampler2D s, vec2 uv, vec3 w) {
        if (u_triplanar == 0) return texture(s, uv);
        return texture(s, v_wpos.yz * u_tpm) * w.x
             + texture(s, v_wpos.xz * u_tpm) * w.y
             + texture(s, v_wpos.xy * u_tpm) * w.z;
    }

    // Cheap hash-based value noise + fBm in 3D — enough for grain/marble at this range.
    float vhash(vec3 p){ p = fract(p * 0.3183099 + 0.1); p *= 17.0; return fract(p.x * p.y * p.z * (p.x + p.y + p.z)); }
    float vnoise(vec3 x){
        vec3 i = floor(x); vec3 f = fract(x); f = f * f * (3.0 - 2.0 * f);
        return mix(mix(mix(vhash(i+vec3(0,0,0)), vhash(i+vec3(1,0,0)), f.x),
                       mix(vhash(i+vec3(0,1,0)), vhash(i+vec3(1,1,0)), f.x), f.y),
                   mix(mix(vhash(i+vec3(0,0,1)), vhash(i+vec3(1,0,1)), f.x),
                       mix(vhash(i+vec3(0,1,1)), vhash(i+vec3(1,1,1)), f.x), f.y), f.z);
    }
    float fbm(vec3 p){
        float a = 0.5, s = 0.0, norm = 0.0;
        for (int i = 0; i < 8; i++) {
            if (float(i) >= u_detail) break;
            s += a * vnoise(p); norm += a; p *= 2.0; a *= u_prough;
        }
        return norm > 0.0 ? s / norm : s;
    }
    // The raw pattern FIELD at a world point, 0..1. Colour, roughness and the bump all read this
    // one function — which is the whole point. A material whose gloss and relief vary with its own
    // grain reads as a material; three unrelated maps read as a picture printed on plastic.
    float proc_field(vec3 wp){
        vec3 p = wp * u_pscale;
        if (u_proc == 2) {          // marble — fbm turbulence folded through a sine band
            return 0.5 + 0.5 * sin((p.x + p.y) * 0.6 + fbm(p) * 6.2831);
        } else if (u_proc == 4) {   // checker — hard cells in world space
            vec3 c = floor(p);
            return mod(c.x + c.y + c.z, 2.0);
        }
        return fbm(p);              // 1 = wood (anisotropic scale does the work), 3 = plain noise
    }

    // Map the field through the two-stop ramp + contrast. The ramp colours are AUTHORED sRGB (that
    // is what the colour pickers show), so they decode to linear here — image textures get the same
    // treatment for free from their SRGB8_ALPHA8 upload.
    float proc_ramp_t(float val){
        float t = smoothstep(u_ramp.x, u_ramp.y, val);
        return clamp((t - 0.5) * u_pcontrast + 0.5, 0.0, 1.0);
    }
    vec3 proc_color(float f){ return srgb_to_lin(mix(u_col_a, u_col_b, proc_ramp_t(f))); }

    // Treat the field as a height map and tilt the normal by its gradient, projected into the
    // surface plane. Central differences at a step tied to the PATTERN's own period, not to the
    // model, so the relief looks the same on a door and on a wall.
    vec3 proc_bump(vec3 N, vec3 wp, float f0){
        if (u_bump <= 0.0 || u_proc == 0) return N;
        float e = 0.35 / max(max(u_pscale.x, u_pscale.y), max(u_pscale.z, 1e-3));
        vec3 g = vec3(proc_field(wp + vec3(e, 0.0, 0.0)) - f0,
                      proc_field(wp + vec3(0.0, e, 0.0)) - f0,
                      proc_field(wp + vec3(0.0, 0.0, e)) - f0);
        g -= N * dot(N, g);          // only the in-plane part tilts a surface
        return normalize(N - g * (u_bump * 4.0));
    }

    // ── Cook-Torrance GGX ────────────────────────────────────────────────────────────────────
    // Replaces the Blinn-Phong `pow(dot(N,H), shininess)` this shader used to run, which had no
    // Fresnel, no energy conservation, and no way for `metallic` to mean anything.
    const float PI = 3.14159265359;
    float d_ggx(float NoH, float a) {
        float a2 = a * a;
        float d = NoH * NoH * (a2 - 1.0) + 1.0;
        return a2 / max(PI * d * d, 1e-7);
    }
    // Height-correlated Smith visibility (the G term already divided by 4·NoV·NoL).
    float v_smith(float NoV, float NoL, float a) {
        float a2 = a * a;
        float sv = NoL * sqrt(NoV * NoV * (1.0 - a2) + a2);
        float sl = NoV * sqrt(NoL * NoL * (1.0 - a2) + a2);
        return 0.5 / max(sv + sl, 1e-5);
    }
    vec3 f_schlick(vec3 f0, float u) { return f0 + (vec3(1.0) - f0) * pow(clamp(1.0 - u, 0.0, 1.0), 5.0); }

    // The environment seen along a reflection ray, blurred by roughness. A prefiltered cubemap mip
    // chain is the textbook answer; this interpolates between the two limits it would produce —
    // the mirror direction (rough 0, sun disc and all) and the cosine-lobe average about that
    // direction (rough 1, which is exactly what the SH ambient already stores). It costs no
    // render target and no upload, and the middle of the range is the part nobody can pick out.
    vec3 env_sample(vec3 R, float rough) {
        // With an HDR environment loaded nothing has to be approximated: the prefiltered chain
        // already holds the real GGX convolution at every roughness, so this is one fetch.
        if (u_env_on == 1) return env_glossy(R, rough);
        return mix(sky_with_sun(R), sh_ambient(R), clamp(rough * 1.4, 0.0, 1.0));
    }

    // The fixed studio key light, when there is no daylight. Kept in one place because the shader
    // has to reproduce the CPU's `0.35 + 0.65·|n·d|` split to know which part of the baked shade is
    // ambient — that share, and only that share, is what ambient occlusion may darken.
    const vec3 STUDIO_DIR = vec3(0.35, 0.25, 0.9);

    // Output alpha = the vertex opacity × the image's own alpha channel. Opaque draws run with
    // blending OFF, so this alpha is simply ignored there; only the blended textured pass uses it.
    void main() {
        // Geometric normal from screen-space derivatives (no per-vertex normal needed), faced to
        // the viewer so two-sided surfaces light correctly.
        vec3 Ng = normalize(cross(dFdx(v_wpos), dFdy(v_wpos)));
        vec3 V = normalize(u_cam - v_wpos);
        if (dot(Ng, V) < 0.0) Ng = -Ng;
        vec3 N = Ng;

        // TEXTURE COORDINATES. Either the mesh's own, or a world-space TRIPLANAR projection at a
        // real tiles-per-metre. Architecture here is mostly CSG extruded from a plan and carries no
        // meaningful UVs at all, so world-space mapping is the only way a brick or a floor tile can
        // be given a believable physical SIZE rather than "one image per face".
        vec2 uv = v_uv;
        vec3 twt = vec3(0.0);
        if (u_triplanar == 1) {
            twt = pow(abs(Ng), vec3(4.0));      // sharp blend: seams only in a narrow band
            twt /= max(twt.x + twt.y + twt.z, 1e-4);
            // One dominant-axis frame for anything needing a single consistent tangent basis.
            uv = (twt.x >= twt.y && twt.x >= twt.z) ? v_wpos.yz * u_tpm
               : (twt.y >= twt.z)                   ? v_wpos.xz * u_tpm
                                                    : v_wpos.xy * u_tpm;
        }

        float field = (u_proc > 0) ? proc_field(v_wpos) : 0.0;
        vec4 t = (u_proc > 0) ? vec4(proc_color(field), 1.0) : tri_or_uv(u_img, uv, twt);
        // Clay: flat grey, keep t.a for glass. Image albedo arrives already linear — the sampler
        // decodes it, because albedo textures upload as SRGB8_ALPHA8 (data maps do not).
        vec3 albedo = u_clay == 1 ? srgb_to_lin(vec3(0.62)) : t.rgb;

        // Tangent-space normal map via a derivative cotangent frame (Schüler) — no stored tangents.
        if (u_has_nrm == 1) {
            vec3 dp1 = dFdx(v_wpos), dp2 = dFdy(v_wpos);
            vec2 duv1 = dFdx(uv), duv2 = dFdy(uv);
            vec3 dp2perp = cross(dp2, Ng), dp1perp = cross(Ng, dp1);
            vec3 T = dp2perp * duv1.x + dp1perp * duv2.x;
            vec3 B = dp2perp * duv1.y + dp1perp * duv2.y;
            float invmax = inversesqrt(max(dot(T, T), dot(B, B)));
            mat3 TBN = mat3(T * invmax, B * invmax, Ng);
            vec3 nm = texture(u_nrm, uv).xyz * 2.0 - 1.0;
            N = normalize(TBN * nm);
        }
        // …and the procedural's own relief, from the field that already coloured it.
        N = proc_bump(N, v_wpos, field);

        // ROUGHNESS, in priority order: a map, then the procedural's own lo→hi range, then the
        // material's scalar. The middle one is what makes a procedural stop looking like a picture:
        // dark grain is rougher than light, so the highlight breaks up along the timber.
        // A pattern whose two ends are EQUAL does not vary its finish, and then the material's own
        // roughness slider is the one that means something — which is also how every pre-Phase-3
        // material loads, so nothing already authored changes.
        float rough = u_rough_base;
        if (u_has_rough == 1)                                        rough = tri_or_uv(u_rough, uv, twt).r;
        else if (u_proc > 0 && abs(u_rough_hi - u_rough_lo) > 0.001) rough = mix(u_rough_lo, u_rough_hi, proc_ramp_t(field));
        rough = clamp(rough, 0.03, 1.0);
        float metallic = (u_has_metal == 1) ? tri_or_uv(u_metal, uv, twt).r * u_metallic : u_metallic;
        // A baked AO map darkens the ambient only — same rule the screen-space pass follows.
        float ao_map = (u_has_ao == 1) ? tri_or_uv(u_aomap, uv, twt).r : 1.0;

        // Specular F0: conductors reflect their own colour, dielectrics a few percent set by IOR.
        float f0d = (u_ior - 1.0) / (u_ior + 1.0);
        f0d = clamp(f0d * f0d, 0.0, 0.25);
        vec3 f0 = mix(vec3(f0d), albedo, metallic);
        vec3 diff = albedo * (1.0 - metallic);
        float a = max(rough * rough, 1e-3);   // Disney-style perceptual → GGX alpha
        float NoV = max(dot(N, V), 1e-4);

        // The reflection ray, and the split-sum weights that say how much of F0 survives at this
        // roughness and viewing angle (Karis's fit — see env.rs).
        vec3 R = reflect(-V, N);
        vec2 ab = env_brdf(rough, NoV);
        vec3 env_w = (f0 * ab.x + ab.y) * clamp(u_reflect, 0.0, 1.0);

        vec3 direct;    // sun + specular + emission — AO must NOT touch this
        vec3 ambient;   // the sky's diffuse contribution — AO's whole remit
        // The key light, resolved the same way by both branches so the clearcoat and sheen lobes
        // below can be written ONCE. Without this they would have to be duplicated, and the studio
        // copy would drift from the daylight copy the first time either was touched.
        vec3  Ldir;     // direction TO the light
        vec3  Lrad;     // its radiance
        float shf;      // shadow factor
        vec3  amb_irr;  // irradiance the ambient term is built from (already occluded by any AO map)
        if (u_sun_on == 1) {
            // Real daylight. `u_sun_col` is calibrated as irradiance/π (which is why the specular
            // carries the π back), and the ambient is now the sky's own irradiance projected onto
            // spherical harmonics — a north wall and a south wall genuinely differ, which the old
            // two-colour `mix(ground, sky, n.z)` fill could not express.
            float sh = (u_shadow_on == 1) ? shadow_lit(v_wpos) : 1.0;
            float NoL = max(dot(N, u_sun_dir), 0.0);
            ambient = diff * sh_ambient(N) * ao_map;
            direct = diff * (u_sun_col * NoL * sh);
            vec3 H = normalize(u_sun_dir + V);
            vec3 F = f_schlick(f0, max(dot(V, H), 0.0));
            direct += F * (d_ggx(max(dot(N, H), 0.0), a) * v_smith(NoV, NoL, a) * NoL * sh * PI) * u_sun_col;
            // Environment specular — the sky itself, reflected. This is what gives a metal
            // something to be a metal ABOUT; before it there was nothing to mirror and a chrome
            // surface rendered as grey plastic no matter what the material said.
            direct += env_sample(R, rough) * env_w;
            Ldir = u_sun_dir; Lrad = u_sun_col; shf = sh;
            amb_irr = sh_ambient(N) * ao_map;
        } else {
            // Studio mode (no daylight): the baked scalar stands in for irradiance. Split it the
            // same way the CPU built it, `0.35 + 0.65·|n·d|`, so the constant fill is separated
            // from the key light and only the fill is occludable.
            float k = abs(dot(N, normalize(STUDIO_DIR)));
            float ambf = 0.35 / (0.35 + 0.65 * k);
            ambient = diff * v_shade * ambf * ao_map;
            direct = diff * v_shade * (1.0 - ambf);
            direct += vec3(v_shade) * env_w;
            Ldir = normalize(STUDIO_DIR); Lrad = vec3(v_shade * (1.0 - ambf)); shf = 1.0;
            amb_irr = vec3(v_shade * ambf * ao_map);
        }

        // ---- CLEARCOAT + SHEEN ------------------------------------------
        // Both are SPECULAR, so both ride on `direct`: ambient occlusion has no business dimming
        // a reflection of the window in a lacquered tabletop.
        if (u_coat > 0.0) {
            // The coat took some light before the material underneath ever saw it. Applied HERE,
            // between the base and the coat's own lobes, so it dims the base and not itself —
            // otherwise a strong clearcoat would darken the very reflection it is adding.
            float loss = u_coat * (0.04 + 0.96 * pow(1.0 - NoV, 5.0));
            direct *= (1.0 - loss);
            ambient *= (1.0 - loss);

            // Reflected about the GEOMETRIC normal, not the bumped one. Varnish fills the grain,
            // which is exactly what separates a polished tabletop — timber visible underneath, the
            // window reflected as one clean shape — from timber with a shiny bump map.
            float ca = max(u_coat_rough * u_coat_rough, 1e-3);
            float NcV = max(dot(Ng, V), 1e-4);
            float NcL = max(dot(Ng, Ldir), 0.0);
            vec3  Hc  = normalize(Ldir + V);
            float Fc  = (0.04 + 0.96 * pow(1.0 - max(dot(V, Hc), 0.0), 5.0)) * u_coat;
            direct += vec3(d_ggx(max(dot(Ng, Hc), 0.0), ca) * v_smith(NcV, NcL, ca) * NcL * shf * PI * Fc) * Lrad;
            vec2 cab = env_brdf(u_coat_rough, NcV);
            direct += env_sample(reflect(-V, Ng), u_coat_rough)
                    * ((0.04 * cab.x + cab.y) * u_coat * clamp(u_reflect, 0.0, 1.0));
        }
        if (u_sheen > 0.0) {
            // Disney's retroreflective term — a Fresnel-shaped rim peaking where the half vector
            // grazes the view, which is where a nap of fibres actually catches the light.
            vec3  Hs   = normalize(Ldir + V);
            float FH   = pow(clamp(1.0 - max(dot(V, Hs), 0.0), 0.0, 1.0), 5.0);
            float NoLs = max(dot(N, Ldir), 0.0);
            direct += u_sheen_tint * (u_sheen * FH * NoLs * shf) * Lrad;
            // …and the same rim under sky light alone. Without this a velvet curtain in a
            // north-facing room — the one place anybody would put one — would be the single
            // situation where the effect disappeared.
            ambient += u_sheen_tint * (u_sheen * pow(1.0 - NoV, 5.0) * 0.5) * amb_irr;
        }
        direct += u_emission;

        // MATERIAL HIGHLIGHT — the material selected in the Materials Factory pulses cyan so the
        // user sees exactly WHERE it is while tuning it. u_hl flags this draw's texture; u_hl_k is
        // the app-driven pulse (0..1). Mixed in LINEAR (the cyan is decoded) so it lands where it
        // used to once the view transform has run. It rides on the direct term so AO cannot eat it.
        if (u_hl == 1) {
            direct = mix(direct + ambient, srgb_to_lin(vec3(0.25, 0.85, 1.0)), u_hl_k * 0.45);
            ambient = vec3(0.0);
        }
        // NOTE: no tone-map here. Every pass writes scene-referred LINEAR light into an RGBA16F
        // target; exposure and the view transform happen once, at the composite (see color.rs).
        float alpha = t.a * v_a;
        frag = vec4(direct, alpha);
        amb_out = vec4(ambient, alpha);
        // The DIFFUSE albedo — what this surface would bounce. `diff` already has the metallic
        // share removed, which is right: a mirror reflects, it does not scatter, and letting a
        // chrome tap bleed its colour onto the wall behind it is a classic screen-space GI tell.
        alb_out = vec4(diff, alpha);
    }
"#;

// Composite: draw the FBO colour texture over an NDC rect (the panel viewport).
const BLIT_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec2 a_pos;   // NDC
    layout(location=1) in vec2 a_uv;
    out vec2 v_uv;
    void main() { v_uv = a_uv; gl_Position = vec4(a_pos, 0.0, 1.0); }
"#;

// The composite is also the DISPLAY TRANSFORM: the FBO holds scene-referred linear light, and this
// is the single point where exposure, the view transform and the sRGB encode are applied — the same
// place Blender puts them, and the reason a bright surface now rolls off instead of clipping.
//
// It is also where the two light buffers are recombined. `u_tex` holds everything that is already
// accounted for (direct sun, specular, emission) and `u_amb` the diffuse ambient; occlusion scales
// only the second. Multiplying AO into the finished image instead — which is what "SSAO" usually
// means in a hurry — would grey out a crease standing in full sunlight.
const BLIT_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    uniform sampler2D u_amb;
    uniform sampler2D u_ao;
    uniform sampler2D u_bloom;
    uniform sampler2D u_ssgi;
    uniform int u_ao_on;
    uniform float u_ssgi_k;
    uniform float u_bloom_k;
    // 1 = `u_tex` is the ACCUMULATION buffer, which already holds composed light (ambient, occlusion
    // and bloom folded in by the TAA resolve). Adding them again here would double them.
    uniform int u_composed;
    FOG_GLSL
    VIEW_GLSL
    void main() {
        vec4 c = texture(u_tex, v_uv);
        vec3 a = texture(u_amb, v_uv).rgb;
        float ao = (u_ao_on == 1) ? texture(u_ao, v_uv).r : 1.0;
        vec3 lit = (u_composed == 1) ? c.rgb : c.rgb + a * ao;
        // Bounced light, before bloom — a bright wall lit by a bounce should be able to bloom, the
        // same as one lit by the sun. `u_ssgi_k` is zeroed when the input is already composed, so
        // the accumulator's bounce is never counted twice.
        if (u_ssgi_k > 0.0) lit += texture(u_ssgi, v_uv).rgb * u_ssgi_k;
        // Bloom is added to SCENE-REFERRED light, before the view transform — a real lens scatters
        // light, it does not brighten pixels. Added after the tone map it would wash out instead of
        // glowing, which is the difference between a luminaire and a smear.
        if (u_bloom_k > 0.0) lit += texture(u_bloom, v_uv).rgb * u_bloom_k;
        // …and so is fog, for the same reason: it is light scattered INTO the ray by the air, and
        // a tone map applied to the sum is not the sum of two tone maps. `u_fog_on` is zeroed when
        // the input is already composed, so the accumulator's fog is never applied twice.
        lit = apply_fog(lit, v_uv);
        frag = vec4(apply_view(lit), c.a);
    }
"#;

// ── BLOOM ────────────────────────────────────────────────────────────────────────────────────
// Light spilling around anything bright enough to overwhelm a lens. For a LIGHTING application
// this is not decoration: without it a luminaire is just a pale rectangle, and the one thing the
// user is designing never reads as a source of light.
//
// Three shaders and an FBO ping-pong — no compute needed. Bright-pass into a half-resolution
// target, downsample five times, then upsample back up ADDING as it goes (the "dual filter" every
// modern engine uses). Successive small blurs compose into a wide one for a fraction of the taps a
// single wide Gaussian would need.

// Bright-pass. The knee matters: a hard cutoff makes a visible contour crawl across a surface as
// the exposure changes, because pixels cross the threshold one at a time.
const BLOOM_PRE_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    uniform sampler2D u_amb;
    uniform sampler2D u_ao;
    uniform int u_ao_on;
    uniform float u_threshold;
    uniform float u_knee;
    void main() {
        vec3 c = texture(u_tex, v_uv).rgb;
        vec3 a = texture(u_amb, v_uv).rgb;
        float ao = (u_ao_on == 1) ? texture(u_ao, v_uv).r : 1.0;
        c += a * ao;
        // NOTHING past this line may be infinite or NaN.
        //
        // The weight below divides by the pixel's own brightness, so an INFINITE pixel gives
        // Inf/Inf = NaN — and NaN does not stay where it started. The downsample averages it into
        // its neighbours and the upsample tent smears it back out, so one blown texel becomes a
        // black block the size of a mip level, with the power-of-two edges to prove it. An HDRI sun
        // reaches this easily: a stock Poly Haven sky peaks at 75360 against half-float's 65504
        // ceiling, so it arrives from the texture already infinite. A strong emissive material can
        // do it too.
        c = min(max(c, vec3(0.0)), vec3(BLOOM_CEILING));
        if (any(isnan(c))) c = vec3(0.0);
        float br = max(c.r, max(c.g, c.b));
        float soft = clamp(br - u_threshold + u_knee, 0.0, 2.0 * u_knee);
        soft = soft * soft / (4.0 * u_knee + 1e-5);
        float w = max(soft, br - u_threshold) / max(br, 1e-5);
        frag = vec4(c * w, 1.0);
    }
"#;

// Downsample: four bilinear taps at the source's texel corners, i.e. a 4×4 box for four fetches.
const BLOOM_DOWN_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    uniform vec2 u_texel;
    void main() {
        vec3 s = texture(u_tex, v_uv + u_texel * vec2(-1.0, -1.0)).rgb
               + texture(u_tex, v_uv + u_texel * vec2( 1.0, -1.0)).rgb
               + texture(u_tex, v_uv + u_texel * vec2(-1.0,  1.0)).rgb
               + texture(u_tex, v_uv + u_texel * vec2( 1.0,  1.0)).rgb;
        frag = vec4(s * 0.25, 1.0);
    }
"#;

// Upsample: a 3×3 tent, blended additively onto the level above.
const BLOOM_UP_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    uniform vec2 u_texel;
    void main() {
        vec3 s = texture(u_tex, v_uv + u_texel * vec2(-1.0, -1.0)).rgb * 1.0
               + texture(u_tex, v_uv + u_texel * vec2( 0.0, -1.0)).rgb * 2.0
               + texture(u_tex, v_uv + u_texel * vec2( 1.0, -1.0)).rgb * 1.0
               + texture(u_tex, v_uv + u_texel * vec2(-1.0,  0.0)).rgb * 2.0
               + texture(u_tex, v_uv).rgb                              * 4.0
               + texture(u_tex, v_uv + u_texel * vec2( 1.0,  0.0)).rgb * 2.0
               + texture(u_tex, v_uv + u_texel * vec2(-1.0,  1.0)).rgb * 1.0
               + texture(u_tex, v_uv + u_texel * vec2( 0.0,  1.0)).rgb * 2.0
               + texture(u_tex, v_uv + u_texel * vec2( 1.0,  1.0)).rgb * 1.0;
        frag = vec4(s / 16.0, 1.0);
    }
"#;

// ── TEMPORAL ACCUMULATION ────────────────────────────────────────────────────────────────────
// While nothing in the frame changes, keep re-rendering it with a sub-pixel camera jitter and
// average the results. Sixteen samples of a still frame cost sixteen ordinary frames — about a
// quarter of a second — and buy anti-aliasing no single-sample raster can reach, because the
// samples land at sixteen positions inside every pixel rather than one.
//
// This is an ACCUMULATOR, not a reprojecting TAA. There is no motion vector, no neighbourhood
// clamp and no history rejection heuristic: the moment ANY input to the frame changes, the
// history is thrown away and the next frame starts again from a single clean sample. That makes
// it incapable of the ghosting and smearing that game TAA is notorious for — the failure mode is
// only ever "no accumulation yet", which looks exactly like today's image.
//
// The averaging happens in SCENE-REFERRED LINEAR light, before the view transform. Averaging
// display values would be wrong: AgX is a curve, and the mean of a curve is not the curve of the
// mean. It matters more later than now — a noisy signal (soft shadows, screen-space GI) averaged
// after the tone map converges to the wrong answer, not merely a slightly different one.
const TAA_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    uniform sampler2D u_amb;
    uniform sampler2D u_ao;
    uniform sampler2D u_bloom;
    uniform sampler2D u_hist;
    uniform sampler2D u_ssgi;
    uniform int u_ao_on;
    uniform float u_ssgi_k;
    uniform float u_bloom_k;
    // Weight of THIS frame: 1.0 on the first sample (history is garbage), 1/(n+1) after, which
    // makes the running result the exact unweighted mean of every sample so far.
    uniform float u_blend;
    FOG_GLSL
    void main() {
        vec4 c = texture(u_tex, v_uv);
        vec3 a = texture(u_amb, v_uv).rgb;
        float ao = (u_ao_on == 1) ? texture(u_ao, v_uv).r : 1.0;
        vec3 lit = c.rgb + a * ao;
        if (u_ssgi_k > 0.0) lit += texture(u_ssgi, v_uv).rgb * u_ssgi_k;
        if (u_bloom_k > 0.0) lit += texture(u_bloom, v_uv).rgb * u_bloom_k;
        lit = apply_fog(lit, v_uv);
        vec4 cur = vec4(lit, c.a);
        frag = (u_blend >= 1.0) ? cur : mix(texture(u_hist, v_uv), cur, u_blend);
    }
"#;

// The fog uniforms + the depth-to-world reconstruction, shared by the composite and the
// accumulation resolve so the two compose a frame identically. The integral itself lives in
// `crate::env::FOG_GLSL`, next to the settings that drive it, and is spliced in below.
const FOG_BLOCK: &str = r#"
    uniform sampler2D u_fog_depth;
    uniform mat4 u_fog_inv_vp;
    uniform vec3 u_fog_cam;
    uniform vec3 u_fog_col;
    uniform float u_fog_density;
    uniform float u_fog_base;
    uniform float u_fog_falloff;
    uniform int u_fog_on;
    FOG_GLSL_BODY
    vec3 apply_fog(vec3 lit, vec2 uv) {
        if (u_fog_on != 1) return lit;
        float d = texture(u_fog_depth, uv).r;
        // The far plane is SKY, not a surface 10 km away. Fogging it would drag the whole
        // background towards the fog colour and flatten the very thing distance is measured
        // against — the sky already IS the haze, drawn properly.
        if (d >= 0.999999) return lit;
        vec4 p = u_fog_inv_vp * vec4(uv * 2.0 - 1.0, d * 2.0 - 1.0, 1.0);
        vec3 world = p.xyz / p.w;
        float t = fog_transmittance(u_fog_cam, world);
        return lit * t + u_fog_col * (1.0 - t);
    }
"#;

/// The whole fog block with the integral spliced in — what `FOG_GLSL` expands to in a shader.
fn fog_glsl() -> String {
    FOG_BLOCK.replace("FOG_GLSL_BODY", crate::env::FOG_GLSL)
}

/// Attachment 2 of the G-buffer: the surface's DIFFUSE ALBEDO — what it would bounce.
///
/// Screen-space GI cannot work without it. Every other buffer holds light that has already been
/// multiplied by albedo (`ambient` is albedo × irradiance, inseparably), so a bounce computed from
/// them alone would land equally on a white floor and a black one. The colour of what light picks
/// up on its way is the entire effect.
///
/// EVERY program that draws into the offscreen FBO must declare and write this. GLSL leaves an
/// undeclared output's attachment undefined rather than zero, so a shader that forgets it does not
/// fail to compile — it fills the albedo buffer with whatever the tile memory held, and GI bounces
/// garbage off it.
const ALBEDO_OUT_GLSL: &str = "layout(location=2) out vec4 alb_out;";

// The SKY as the backdrop. Drawn first, over the whole viewport, with depth testing off — the
// geometry then paints over it. The ray is reconstructed from the inverse view-projection, so it is
// the same camera the model is drawn with and the horizon sits where it should.
const SKY_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    layout(location=0) out vec4 frag;
    layout(location=1) out vec4 amb_out;
    ALBEDO_OUT_GLSL
    uniform mat4 u_inv_vp;
    uniform vec3 u_cam;
    SKY_GLSL
    void main() {
        vec4 p = u_inv_vp * vec4(v_uv * 2.0 - 1.0, 1.0, 1.0);
        vec3 dir = normalize(p.xyz / p.w - u_cam);
        frag = vec4(sky_with_sun(dir), 1.0);
        amb_out = vec4(0.0, 0.0, 0.0, 1.0);
        // The sky is not a surface and has no albedo. Zero marks it as BACKGROUND, which is what
        // stops screen-space GI from treating the horizon as an enormous bounce card — the sky
        // already lights the scene properly through its spherical harmonics.
        alb_out = vec4(0.0);
    }
"#;

// SCREEN-SPACE AMBIENT OCCLUSION. Sky lighting alone makes a render *flatter*, not rounder: every
// point gets the sky's full irradiance for its normal, whether it sits on an open wall or wedged in
// the corner behind a sofa. This estimates how much of the hemisphere is actually blocked, from the
// depth buffer alone — no extra geometry pass, and it covers every draw kind automatically.
//
// World space rather than view space on purpose: the radius is then a length in metres that means
// the same thing at any zoom, which is what lets it be a user setting instead of a magic number.
const SSAO_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_depth;
    uniform mat4 u_vp;
    uniform mat4 u_inv_vp;
    uniform vec3 u_cam;
    uniform float u_radius;
    uniform float u_strength;

    vec3 world_at(vec2 uv) {
        float d = texture(u_depth, uv).r;
        vec4 p = u_inv_vp * vec4(uv * 2.0 - 1.0, d * 2.0 - 1.0, 1.0);
        return p.xyz / p.w;
    }

    void main() {
        float d0 = texture(u_depth, v_uv).r;
        if (d0 >= 0.99999) { frag = vec4(1.0); return; }   // background — nothing to occlude
        vec3 P = world_at(v_uv);
        // The normal comes from the depth buffer's own screen-space derivatives, so no normal
        // target is needed. It is faceted at silhouettes; the blur that follows hides that.
        vec3 N = normalize(cross(dFdx(P), dFdy(P)));
        if (dot(N, u_cam - P) < 0.0) N = -N;
        vec3 up = abs(N.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
        vec3 T = normalize(cross(N, up));
        vec3 B = cross(N, T);
        // Per-pixel spin: 16 samples that are differently oriented at every pixel read as far more
        // than 16 once the blur averages a neighbourhood of them.
        float rot = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) * 6.2831853;

        float occ = 0.0;
        for (int i = 0; i < 16; i++) {
            float a = (float(i) + 0.5) / 16.0;
            float phi = a * 6.2831853 * 5.0 + rot;
            float st = sqrt(a), ct = sqrt(1.0 - a);
            float rr = mix(0.15, 1.0, a * a);   // bunch the samples near the origin: contact first
            vec3 dir = T * (cos(phi) * st) + B * (sin(phi) * st) + N * ct;
            vec3 sp = P + dir * (u_radius * rr);
            vec4 cp = u_vp * vec4(sp, 1.0);
            if (cp.w <= 0.0) continue;
            vec2 su = cp.xy / cp.w * 0.5 + 0.5;
            if (su.x < 0.0 || su.x > 1.0 || su.y < 0.0 || su.y > 1.0) continue;
            float sample_d = length(sp - u_cam);
            float scene_d = length(world_at(su) - u_cam);
            float bias = 0.02 + 0.01 * sample_d;   // scales with distance: no acne when dollied out
            if (scene_d < sample_d - bias) {
                // Range check — a wall forty metres behind a table must not shadow it.
                occ += smoothstep(0.0, 1.0, u_radius / max(sample_d - scene_d, 1e-4));
            }
        }
        frag = vec4(clamp(1.0 - u_strength * occ / 16.0, 0.0, 1.0));
    }
"#;

// SCREEN-SPACE GLOBAL ILLUMINATION — one bounce of coloured light between surfaces.
//
// The renderer's only indirect light is the sky's irradiance, which is the same in every direction
// a normal can face and knows nothing about the room. So a red rug lights nothing, a white wall
// beside a window bounces nothing onto the ceiling, and every interior comes out looking like the
// objects were composited in rather than standing in the space. This gathers the light actually
// leaving nearby surfaces and lets it land.
//
// It is an APPROXIMATION and cannot be otherwise: it can only gather from surfaces the camera can
// already see, so a bounce off a wall behind you does not exist, and one leaving the frame fades
// out. Two things keep it from looking broken rather than merely incomplete:
//
//   * the EMITTER's normal is reconstructed and must face back at the receiver. Without that,
//     light pours through walls — the single most recognisable screen-space GI failure.
//   * the receiver's ALBEDO multiplies the result here, not at the composite. Bounced light is
//     coloured by what it lands on; a term that ignored albedo would light a black floor and a
//     white one identically, which reads as fog rather than as bounce.
const SSGI_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_depth;
    uniform sampler2D u_lit;    // attachment 0 — direct + specular + emission
    uniform sampler2D u_amb;    // attachment 1 — the diffuse ambient
    uniform sampler2D u_alb;    // attachment 2 — diffuse albedo
    uniform sampler2D u_ao;
    uniform int   u_ao_on;
    uniform mat4  u_vp;
    uniform mat4  u_inv_vp;
    uniform vec3  u_cam;
    uniform float u_radius;
    uniform float u_strength;
    uniform vec2  u_texel;
    uniform int   u_frame;

    vec3 world_at(vec2 uv) {
        float d = texture(u_depth, uv).r;
        vec4 p = u_inv_vp * vec4(uv * 2.0 - 1.0, d * 2.0 - 1.0, 1.0);
        return p.xyz / p.w;
    }
    // The light LEAVING a pixel, composed exactly as the composite composes it. Anything else and
    // the bounce would carry a different picture than the one on screen.
    vec3 radiance_at(vec2 uv) {
        vec3 c = texture(u_lit, uv).rgb;
        vec3 a = texture(u_amb, uv).rgb;
        float ao = (u_ao_on == 1) ? texture(u_ao, uv).r : 1.0;
        return c + a * ao;
    }
    // The emitter's normal, by finite difference on the depth buffer. `dFdx` cannot be used for
    // this: the point is fetched from a texture, so its derivative across the quad is meaningless.
    vec3 normal_at(vec2 uv, vec3 c) {
        vec3 dx = world_at(uv + vec2(u_texel.x, 0.0)) - c;
        vec3 dy = world_at(uv + vec2(0.0, u_texel.y)) - c;
        return normalize(cross(dx, dy));
    }

    void main() {
        float d0 = texture(u_depth, v_uv).r;
        if (d0 >= 0.99999) { frag = vec4(0.0); return; }   // background
        vec3 alb = texture(u_alb, v_uv).rgb;
        // Nothing to bounce with: background, glass, or a UI swatch. All three write zero albedo.
        if (alb.r + alb.g + alb.b <= 0.0) { frag = vec4(0.0); return; }

        vec3 P = world_at(v_uv);
        vec3 N = normalize(cross(dFdx(P), dFdy(P)));
        if (dot(N, u_cam - P) < 0.0) N = -N;
        vec3 up = abs(N.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
        vec3 T = normalize(cross(N, up));
        vec3 B = cross(N, T);
        // Spun per PIXEL and per FRAME. Per-pixel alone would make temporal accumulation average
        // the same eight directions sixteen times over and converge to its own bias; varying with
        // the frame is what turns the accumulation into a real integral.
        float rot = fract(sin(dot(gl_FragCoord.xy + vec2(float(u_frame) * 17.0),
                                 vec2(12.9898, 78.233))) * 43758.5453) * 6.2831853;

        vec3 gi = vec3(0.0);
        for (int i = 0; i < 8; i++) {
            float a = (float(i) + 0.5) / 8.0;
            float phi = a * 6.2831853 * 3.0 + rot;
            float st = sqrt(a), ct = sqrt(1.0 - a);
            vec3 dir = T * (cos(phi) * st) + B * (sin(phi) * st) + N * ct;
            vec4 cp = u_vp * vec4(P + dir * (u_radius * mix(0.25, 1.0, a)), 1.0);
            if (cp.w <= 0.0) continue;
            vec2 su = cp.xy / cp.w * 0.5 + 0.5;
            if (su.x < 0.0 || su.x > 1.0 || su.y < 0.0 || su.y > 1.0) continue;
            if (texture(u_depth, su).r >= 0.99999) continue;   // sky is not a bounce card
            vec3 Q = world_at(su);
            vec3 to = Q - P;
            float dist = length(to);
            if (dist < 1e-3 || dist > u_radius) continue;
            to /= dist;
            // Both of these are REJECTIONS, not weights.
            //
            // The directions above are drawn cosine-weighted (cos = sqrt(1-a)), so the receiver's
            // cosine and the 1/pi of the diffuse BRDF are already carried by the sampling: the
            // estimator of outgoing radiance is simply albedo times the MEAN of what those
            // directions saw. Multiplying by the cosine again applies it twice, which is what made
            // the whole effect four-odd times too dark to see.
            if (dot(N, to) <= 0.0) continue;                    // behind the receiver
            if (-dot(normal_at(su, Q), to) <= 0.0) continue;    // the far side of a wall
            // A window, not an attenuation: full strength out to most of the radius, then a fade
            // so a surface does not pop as it crosses the boundary.
            gi += radiance_at(su) * smoothstep(1.0, 0.65, dist / u_radius);
        }
        // The mean over ALL directions, including the ones that found nothing — a direction that
        // escaped saw the sky, and the sky is already lighting this surface through its spherical
        // harmonics. Counting it here as well would be double-counting.
        frag = vec4(alb * gi * (u_strength / 8.0), 1.0);
    }
"#;

// A 4×4 box blur over an RGB target — the twin of `BLUR_FS`, which is single-channel. The GI
// gather is deliberately noisy (eight jittered directions), and this is what turns that noise back
// into the broad, soft term bounced light actually is.
const BLUR_RGB_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_src;
    void main() {
        vec2 tx = 1.0 / vec2(textureSize(u_src, 0));
        vec3 s = vec3(0.0);
        for (int y = -2; y <= 1; y++)
            for (int x = -2; x <= 1; x++)
                s += texture(u_src, v_uv + vec2(x, y) * tx).rgb;
        frag = vec4(s / 16.0, 1.0);
    }
"#;

// A plain 4×4 box blur over the AO buffer — the per-pixel rotation above trades banding for noise,
// and this is what turns that noise back into a smooth term.
const BLUR_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_ao;
    void main() {
        vec2 tx = 1.0 / vec2(textureSize(u_ao, 0));
        float s = 0.0;
        for (int y = -2; y <= 1; y++)
            for (int x = -2; x <= 1; x++)
                s += texture(u_ao, v_uv + vec2(x, y) * tx).r;
        frag = vec4(s / 16.0);
    }
"#;

/// `&[f32]` as raw bytes for a GL upload. Same lifetime, same alignment (4 ≥ 1), no copy.
fn bytemuck_cast(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and no invalid bit patterns, and the result borrows `v`, so the
    // slice cannot outlive the data or be written through.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// One HDR environment ready for upload: the prefiltered chain and a version that changes whenever
/// the map does, so the renderer can tell "same environment" from "new environment" without
/// comparing megabytes of pixels.
pub struct EnvUpload<'a> {
    pub chain: &'a [crate::env_map::EnvMip],
    /// The environment at FULL resolution, for the backdrop and near-mirror reflection. The chain's
    /// own level 0 is capped to keep it a clean mip chain, which is too soft for something looked
    /// at directly.
    pub source: &'a crate::env_map::EnvMap,
    pub version: u64,
}

/// Texture unit the HDR environment lives on, in every program that includes
/// [`crate::env::SKY_GLSL`]. Units 0–5 are already spoken for (albedo, shadow, and the four PBR
/// maps), and one number shared by all three programs is one fewer thing to get out of step.
const ENV_UNIT: i32 = 6;

/// The uniform locations [`crate::env::SKY_GLSL`] declares. Two programs include that source — the
/// backdrop and the surface shader — and a uniform belongs to a *program*, not to the text, so the
/// locations have to be resolved and set once per program. Bundling them keeps the two in step.
#[derive(Default)]
struct SkyUniforms {
    perez: Option<glow::UniformLocation>,
    perez_norm: Option<glow::UniformLocation>,
    zenith_xy: Option<glow::UniformLocation>,
    scale: Option<glow::UniformLocation>,
    sun: Option<glow::UniformLocation>,
    ground: Option<glow::UniformLocation>,
    sun_col: Option<glow::UniformLocation>,
    on: Option<glow::UniformLocation>,
    sh: Option<glow::UniformLocation>,
    env: Option<glow::UniformLocation>,
    env_bg: Option<glow::UniformLocation>,
    env_on: Option<glow::UniformLocation>,
    env_rot: Option<glow::UniformLocation>,
    env_strength: Option<glow::UniformLocation>,
}

impl SkyUniforms {
    unsafe fn locate(gl: &glow::Context, prog: glow::Program) -> Self {
        Self {
            perez: gl.get_uniform_location(prog, "u_perez"),
            perez_norm: gl.get_uniform_location(prog, "u_perez_norm"),
            zenith_xy: gl.get_uniform_location(prog, "u_zenith_xy"),
            scale: gl.get_uniform_location(prog, "u_sky_scale"),
            sun: gl.get_uniform_location(prog, "u_sky_sun"),
            ground: gl.get_uniform_location(prog, "u_sky_ground"),
            sun_col: gl.get_uniform_location(prog, "u_sky_sun_col"),
            on: gl.get_uniform_location(prog, "u_sky_on"),
            sh: gl.get_uniform_location(prog, "u_sh"),
            env: gl.get_uniform_location(prog, "u_env"),
            env_bg: gl.get_uniform_location(prog, "u_env_bg"),
            env_on: gl.get_uniform_location(prog, "u_env_on"),
            env_rot: gl.get_uniform_location(prog, "u_env_rot"),
            env_strength: gl.get_uniform_location(prog, "u_env_strength"),
        }
    }

    /// Push one frame's environment. The program must already be bound.
    ///
    /// `env_unit` is the texture unit the HDR environment is bound to — passed in rather than
    /// fixed, because the textured program has albedo/normal/roughness/AO ahead of it and the
    /// scene program does not, so "the next free unit" is a different number in each.
    unsafe fn set(&self, gl: &glow::Context, env: &crate::env::EnvRender, env_unit: i32) {
        let on = env.sky.map(|s| s.valid).unwrap_or(false);
        if let Some(loc) = &self.on {
            gl.uniform_1_i32(Some(loc), on as i32);
        }
        // The HDR environment, when one is loaded. Set on EVERY program that includes SKY_GLSL,
        // including when it is off — a stale `u_env_on` left at 1 on a program that has since lost
        // its texture samples whatever happens to be bound to that unit.
        if let Some(loc) = &self.env {
            gl.uniform_1_i32(Some(loc), env_unit);
        }
        if let Some(loc) = &self.env_bg {
            gl.uniform_1_i32(Some(loc), env_unit + 1);
        }
        if let Some(loc) = &self.env_on {
            gl.uniform_1_i32(Some(loc), env.hdri.is_some() as i32);
        }
        if let Some(loc) = &self.env_rot {
            gl.uniform_1_f32(Some(loc), env.hdri.map(|h| h.rot).unwrap_or(0.0));
        }
        if let Some(loc) = &self.env_strength {
            gl.uniform_1_f32(Some(loc), env.hdri.map(|h| h.strength).unwrap_or(1.0));
        }
        // The SH ambient is uploaded regardless: the studio path leaves it at zero, and a stale
        // set of coefficients on a program that has stopped using them is a real class of bug.
        if let Some(loc) = &self.sh {
            let mut flat = [0.0f32; 27];
            for (i, c) in env.sh.iter().enumerate() {
                flat[i * 3..i * 3 + 3].copy_from_slice(c);
            }
            gl.uniform_3_f32_slice(Some(loc), &flat);
        }
        let Some(sky) = env.sky else { return };
        if let Some(loc) = &self.perez {
            gl.uniform_3_f32_slice(Some(loc), &sky.perez_uniform());
        }
        if let Some(loc) = &self.perez_norm {
            let n = sky.norm_uniform();
            gl.uniform_3_f32(Some(loc), n[0], n[1], n[2]);
        }
        if let Some(loc) = &self.zenith_xy {
            gl.uniform_2_f32(Some(loc), sky.zenith_xy[0], sky.zenith_xy[1]);
        }
        if let Some(loc) = &self.scale {
            gl.uniform_1_f32(Some(loc), sky.scale);
        }
        if let Some(loc) = &self.sun {
            gl.uniform_3_f32(Some(loc), sky.sun_dir.x, sky.sun_dir.y, sky.sun_dir.z);
        }
        if let Some(loc) = &self.ground {
            gl.uniform_3_f32(Some(loc), sky.ground[0], sky.ground[1], sky.ground[2]);
        }
        if let Some(loc) = &self.sun_col {
            gl.uniform_3_f32(Some(loc), sky.sun_col[0], sky.sun_col[1], sky.sun_col[2]);
        }
    }
}

/// Paste the shared GLSL chunks into the textured fragment shader. One function so `ensure_init`
/// and the shader-assembly tests compile the identical text — a test that assembled it slightly
/// differently would be testing nothing.
fn assemble_tex_fs() -> String {
    TEX_FS
        .replace("ALBEDO_OUT_GLSL", ALBEDO_OUT_GLSL)
        .replace("SHADOW_GLSL", &shadow_glsl())
        .replace("SRGB_GLSL", crate::color::SRGB_GLSL)
        .replace("SKY_GLSL", crate::env::SKY_GLSL)
        .replace("ENV_BRDF_GLSL", crate::env::ENV_BRDF_GLSL)
}

/// The other three programs that draw into the offscreen FBO, assembled the same way and in ONE
/// place each — so `ensure_init` and every test compile identical text. A test that assembled the
/// shader slightly differently from the renderer would be testing nothing.
fn assemble_scene_fs() -> String {
    SCENE_FS
        .replace("ALBEDO_OUT_GLSL", ALBEDO_OUT_GLSL)
        .replace("SHADOW_GLSL", &shadow_glsl())
        .replace("SRGB_GLSL", crate::color::SRGB_GLSL)
        .replace("SKY_GLSL", crate::env::SKY_GLSL)
}

fn assemble_transp_fs() -> String {
    TRANSP_FS
        .replace("ALBEDO_OUT_GLSL", ALBEDO_OUT_GLSL)
        .replace("SRGB_GLSL", crate::color::SRGB_GLSL)
}

fn assemble_sky_fs() -> String {
    SKY_FS
        .replace("ALBEDO_OUT_GLSL", ALBEDO_OUT_GLSL)
        .replace("SKY_GLSL", crate::env::SKY_GLSL)
}

/// The assembled textured fragment shader, for cross-module tests that need to check the GLSL
/// against a Rust twin of it (see [`crate::proc_tex`]).
#[cfg(test)]
pub fn tex_fs_for_test() -> String {
    assemble_tex_fs()
}

/// The assembled SCENE fragment shader, so a test can read the lighting constants back out of the
/// source rather than trust that the Rust twin and the GLSL were typed the same.
#[cfg(test)]
pub fn scene_fs_for_test() -> String {
    SCENE_FS
        .replace("SHADOW_GLSL", &shadow_glsl())
        .replace("SRGB_GLSL", crate::color::SRGB_GLSL)
        .replace("SKY_GLSL", crate::env::SKY_GLSL)
}

/// Everything about one [`Scene3dRenderer::render`] call that could change the resulting image,
/// reduced to something comparable with the previous frame's.
///
/// Temporal accumulation is only honest while the image is genuinely unchanged, so "did anything
/// change?" has to be answered from ALL the inputs. Miss one and the viewport silently keeps
/// showing a stale picture — the worst kind of rendering bug, because nothing looks broken.
///
/// That is why this is built INSIDE `render`, out of `render`'s own parameters, rather than by the
/// caller: the renderer cannot forget an argument it was handed. Bulk data (vertices, matrices,
/// ids) is folded into a hash; the small settings structs go in through their `Debug` output,
/// because `Debug` prints every field — so a field added to `ProcParams` or `ColorPipeline` later
/// joins the key automatically instead of quietly falling out of it.
#[derive(Default, PartialEq, Clone, Copy, Debug)]
struct FrameKey {
    hash: u64,
    /// The accumulation buffers are viewport-sized, so a resize invalidates the history outright.
    size: (i32, i32),
}

/// FNV-1a over 64-bit words. Not cryptographic and not trying to be — it only has to notice
/// change, and it runs over every dynamic vertex the frame uploads, so it has to be cheap.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn u64(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x100_0000_01b3);
    }
    fn f32(&mut self, v: f32) {
        self.u64(v.to_bits() as u64);
    }
    fn f32s(&mut self, v: &[f32]) {
        for &x in v {
            self.f32(x);
        }
    }
    fn bytes(&mut self, b: &[u8]) {
        let mut it = b.chunks_exact(8);
        for c in &mut it {
            self.u64(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let mut tail = [0u8; 8];
        let r = it.remainder();
        tail[..r.len()].copy_from_slice(r);
        self.u64(u64::from_le_bytes(tail));
        self.u64(b.len() as u64);
    }
    /// Fold a value in through its `Debug` output, reusing `buf` so this never allocates.
    fn dbg(&mut self, buf: &mut String, v: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        buf.clear();
        let _ = write!(buf, "{v:?}");
        self.bytes(buf.as_bytes());
    }
}

/// The sub-pixel offset of accumulation sample `i`, in pixels, from a Halton (2, 3) sequence.
///
/// Halton rather than random: it fills the pixel evenly at EVERY prefix length, so the image is
/// already well anti-aliased after four samples instead of only after all sixteen.
fn halton_jitter(i: u32) -> (f32, f32) {
    (halton_base(i, 2) - 0.5, halton_base(i, 3) - 0.5)
}

/// Nudge a view-projection matrix by `(jx, jy)` PIXELS on a `w`×`h` viewport.
///
/// The shift has to be proportional to `w` (clip space), not applied after the divide, or objects
/// at different depths would jitter by different amounts and the accumulation would blur the scene
/// instead of anti-aliasing it. In a column-major `[f32; 16]`, row 0 is elements 0, 4, 8, 12 and
/// row 3 (the `w` row) is 3, 7, 11, 15.
fn jitter_mvp(mvp: &[f32; 16], jx: f32, jy: f32, w: i32, h: i32) -> [f32; 16] {
    let (sx, sy) = (2.0 * jx / w.max(1) as f32, 2.0 * jy / h.max(1) as f32);
    let mut m = *mvp;
    for c in 0..4 {
        m[c * 4] += sx * m[c * 4 + 3];
        m[c * 4 + 1] += sy * m[c * 4 + 3];
    }
    m
}

/// Move the sun to a different point on its own disc for accumulation sample `i`, and carry the
/// shadow matrix with it.
///
/// The sun is not a point — it is half a degree wide. That is the whole reason a real shadow edge
/// is crisp under a table leg and soft under a roof eave: the penumbra is the sun's disc projected
/// past the occluder, so it widens with the distance the shadow has to travel. Rendering each
/// accumulation sample from a different point on the disc and averaging reproduces exactly that.
///
/// It gets the WIDENING right, which is what no amount of blurring a shadow map can do — a blur
/// has one width everywhere, so it either smears the contact shadow under a chair leg or leaves
/// the eaves razor-sharp, and usually both at once. And it costs nothing beyond the accumulation
/// that is running anyway: the same sixteen frames that anti-alias the image also integrate the
/// sun's disc.
///
/// The shadow matrix is rotated rather than rebuilt, because the app owns how it was fitted to the
/// scene and this code has no business duplicating that. Rotating the world by `R⁻¹` about the
/// point the light frustum is centred on is exactly equivalent to rotating the light by `R`, and
/// leaves the fit intact — at half a degree the frustum still covers what it covered.
fn jitter_sun(
    dir: [f32; 3], shadow: Option<[f32; 16]>, half_angle: f32, i: u32,
) -> ([f32; 3], Option<[f32; 16]>) {
    let (d2, spin) = sun_disc_sample(dir, half_angle, i);
    let Some(spin) = spin else { return (dir, shadow) };
    (d2, shadow.map(|m| rotate_light(m, spin)))
}

/// The sun's direction for accumulation sample `i`, and the world rotation that takes the light
/// there — split out so a whole set of cascades can be turned by the SAME rotation. Turning each
/// cascade by an independently drawn sample would light each slice of the view from a slightly
/// different sun, and the seam between two cascades would flicker.
///
/// `None` for the rotation means "nothing to do" — a zero-width disc, or a degenerate direction.
fn sun_disc_sample(dir: [f32; 3], half_angle: f32, i: u32) -> ([f32; 3], Option<glam::Quat>) {
    let d = Vec3::from(dir).normalize_or_zero();
    if d == Vec3::ZERO || half_angle <= 0.0 {
        return (dir, None);
    }
    // A point on the disc, area-uniform (the sqrt) so the middle is not over-weighted. Halton
    // again, for the same reason as the pixel jitter: every prefix is already well spread, so the
    // penumbra looks right after four samples rather than only after all sixteen.
    let (h2, h3) = (halton_base(i, 2), halton_base(i, 3));
    let (r, phi) = (h2.sqrt() * half_angle, h3 * std::f32::consts::TAU);
    // Any two vectors perpendicular to the sun. Which two does not matter — the disc is round.
    let seed = if d.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = d.cross(seed).normalize_or_zero();
    let v = d.cross(u);
    let d2 = (d + u * (r * phi.cos()) + v * (r * phi.sin())).normalize_or_zero();
    if d2 == Vec3::ZERO {
        return (dir, None);
    }
    ([d2.x, d2.y, d2.z], Some(glam::Quat::from_rotation_arc(d, d2)))
}

/// Turn a light matrix by `spin`, as if the light itself had moved.
///
/// Rotating the WORLD by `spin⁻¹` about the point the light frustum is centred on is exactly
/// equivalent to rotating the light by `spin`, and it leaves the app's fit intact — which matters,
/// because the app owns how the cascade was framed and this code has no business duplicating that.
/// About the frustum's own centre, not the origin: a scene 200 m from the world origin would
/// otherwise swing bodily out of its own shadow map.
fn rotate_light(m: [f32; 16], spin: glam::Quat) -> [f32; 16] {
    let lm = Mat4::from_cols_array(&m);
    if lm.determinant().abs() < 1e-20 {
        return m; // not invertible — leave it alone rather than corrupt it
    }
    let c = lm.inverse().project_point3(Vec3::ZERO);
    let rot = Mat4::from_quat(spin.inverse());
    (lm * (Mat4::from_translation(c) * rot * Mat4::from_translation(-c))).to_cols_array()
}

/// The `i`-th value of the Halton sequence in `base` — the building block of both jitters.
fn halton_base(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

pub struct Scene3dRenderer {
    inited: bool,
    scene_prog: Option<glow::Program>,
    u_mvp: Option<glow::UniformLocation>,
    u_alpha: Option<glow::UniformLocation>,
    scene_vao: Option<glow::VertexArray>,
    scene_vbo: Option<glow::Buffer>,
    // STATIC opaque batches with their own VBO/VAO, re-uploaded ONLY when their version
    // changes — so a heavy scene (or a dragged mesh) isn't re-sent to the GPU every frame
    // (that 28 MB/frame upload was the idle lag with a 400k-tri import). Slot 0 = the opaque
    // scene, slot 1 = the dragged furniture.
    static_vao: [Option<glow::VertexArray>; 2],
    static_vbo: [Option<glow::Buffer>; 2],
    static_ver: [u64; 2],
    static_len: [i32; 2],
    /// Per-furniture GPU buffers, keyed by (asset,colour). Each furniture mesh is uploaded
    /// ONCE and drawn every frame with just a model matrix — so importing/moving/rotating a
    /// multi-million-triangle piece never CPU-transforms or re-uploads it. Shared by all
    /// instances of the same asset+colour.
    furn_bufs: std::collections::HashMap<u64, (glow::VertexArray, glow::Buffer, i32)>,
    // TRANSPARENT furniture pass: its own program (pos+colour+opacity) and per-mesh GPU buffers,
    // keyed like `furn_bufs` but holding only the translucent triangles of an asset.
    transp_prog: Option<glow::Program>,
    u_transp_mvp: Option<glow::UniformLocation>,
    transp_bufs: std::collections::HashMap<u64, (glow::VertexArray, glow::Buffer, i32)>,
    // TEXTURED pass: its own program (pos+uv+shade), per-mesh GPU buffers keyed like
    // `furn_bufs`, and a cache of uploaded GL images keyed by the app-side texture index.
    tex_prog: Option<glow::Program>,
    u_tex_mvp: Option<glow::UniformLocation>,
    u_tex_img: Option<glow::UniformLocation>,
    u_tex_cam: Option<glow::UniformLocation>,
    u_tex_reflect: Option<glow::UniformLocation>,
    // PROCEDURAL material uniforms — when `u_proc` > 0 the textured shader ignores the bound image
    // and evaluates a world-space noise→ramp pattern (wood grain, marble, …) instead.
    u_tex_proc: Option<glow::UniformLocation>,
    u_tex_col_a: Option<glow::UniformLocation>,
    u_tex_col_b: Option<glow::UniformLocation>,
    u_tex_pscale: Option<glow::UniformLocation>,
    u_tex_detail: Option<glow::UniformLocation>,
    u_tex_prough: Option<glow::UniformLocation>,
    u_tex_pcontrast: Option<glow::UniformLocation>,
    u_tex_ramp: Option<glow::UniformLocation>,
    // Sun/shadow/PBR-map uniforms for the textured program (Texture Phase 2 + daylight).
    u_tex_model: Option<glow::UniformLocation>,
    u_tex_sun_on: Option<glow::UniformLocation>,
    u_tex_sun_dir: Option<glow::UniformLocation>,
    u_tex_sun_col: Option<glow::UniformLocation>,
    u_tex_emission: Option<glow::UniformLocation>,
    /// The HDR environment as ONE 2D texture whose mip levels ARE the roughness chain, plus the
    /// version of the map it was built from so it re-uploads only when the environment changes.
    env_tex: Option<glow::Texture>,
    /// The same environment at FULL resolution — the backdrop and near-mirror reflections.
    env_bg_tex: Option<glow::Texture>,
    env_version: u64,
    /// BLOOM: the bright-pass/downsample/upsample programs, and the pyramid they ping-pong through.
    /// `bloom_tex[0]` is half the viewport, each level half the one above.
    bloom_pre_prog: Option<glow::Program>,
    bloom_down_prog: Option<glow::Program>,
    bloom_up_prog: Option<glow::Program>,
    bloom_tex: Vec<glow::Texture>,
    bloom_size: Vec<(i32, i32)>,
    bloom_fbo: Option<glow::Framebuffer>,
    /// TEMPORAL ACCUMULATION (see [`TAA_FS`]). Two viewport-sized RGBA16F buffers ping-ponged by
    /// the resolve pass, plus the bookkeeping that decides when to start over.
    taa_prog: Option<glow::Program>,
    taa_fbo: Option<glow::Framebuffer>,
    taa_tex: [Option<glow::Texture>; 2],
    /// The size `taa_tex` was allocated at — its own, because `fbo_w`/`fbo_h` have already been
    /// updated to the NEW size by the time the accumulator gets to notice a resize.
    taa_size: (i32, i32),
    /// Which of `taa_tex` holds the CURRENT accumulation (the other is last frame's history).
    taa_cur: usize,
    /// Requested by the caller through [`Scene3dRenderer::set_taa`]; 0 samples = off.
    taa_max: u32,
    /// How many samples the history already holds. 0 = nothing accumulated yet.
    taa_n: u32,
    /// Whether `taa_tex[taa_cur]` actually HOLDS a finished image.
    ///
    /// Separate from `taa_n` because the two can disagree, and the disagreement is catastrophic:
    /// the converged path re-presents the accumulation buffer INSTEAD of drawing the scene, so if
    /// it ever presents a buffer nothing wrote, the viewport goes black and stays black — there is
    /// no next frame that would fix it, because the whole point of that path is not to render one.
    taa_valid: bool,
    /// Everything about the last frame that could change the image — see [`FrameKey`].
    taa_key: FrameKey,
    /// Whether the last frame's key MATCHED the one before it.
    ///
    /// The renderer only asks the caller to keep repainting once it has seen the frame hold still
    /// at least once. Without that condition, a scene whose draw list is rebuilt slightly
    /// differently every frame would repaint → change → repaint forever, spinning the GPU at full
    /// speed on a viewport nobody is touching, and the loop would be driven by the very requests
    /// meant to end it. Costing one frame of latency to rule that out is a good trade.
    taa_stable: bool,
    /// Scratch string the key formats into, reused so a per-frame comparison never allocates.
    taa_dbg: String,
    /// The sky/IBL uniforms of the TEXTURED program (see [`SkyUniforms`]).
    sky_u_tex: SkyUniforms,
    sky_u_scene: SkyUniforms,
    u_tex_hl: Option<glow::UniformLocation>,
    u_tex_hl_k: Option<glow::UniformLocation>,
    u_tex_nrm: Option<glow::UniformLocation>,
    u_tex_rough: Option<glow::UniformLocation>,
    u_tex_metal_map: Option<glow::UniformLocation>,
    u_tex_ao_map: Option<glow::UniformLocation>,
    u_tex_has_nrm: Option<glow::UniformLocation>,
    u_tex_has_rough: Option<glow::UniformLocation>,
    u_tex_has_metal: Option<glow::UniformLocation>,
    u_tex_has_ao: Option<glow::UniformLocation>,
    u_tex_triplanar: Option<glow::UniformLocation>,
    u_tex_tpm: Option<glow::UniformLocation>,
    u_tex_rough_lo: Option<glow::UniformLocation>,
    u_tex_rough_hi: Option<glow::UniformLocation>,
    u_tex_bump: Option<glow::UniformLocation>,
    u_tex_rough_base: Option<glow::UniformLocation>,
    u_tex_coat: Option<glow::UniformLocation>,
    u_tex_coat_rough: Option<glow::UniformLocation>,
    u_tex_sheen: Option<glow::UniformLocation>,
    u_tex_sheen_tint: Option<glow::UniformLocation>,
    u_tex_shadow_on: Option<glow::UniformLocation>,
    u_tex_light_mvp: Option<glow::UniformLocation>,
    u_tex_csm_n: Option<glow::UniformLocation>,
    u_tex_shadow: Option<glow::UniformLocation>,
    u_tex_clay: Option<glow::UniformLocation>,
    u_tex_metallic: Option<glow::UniformLocation>,
    u_tex_ior: Option<glow::UniformLocation>,
    // Scene program: world-model + shadow uniforms.
    u_scene_model: Option<glow::UniformLocation>,
    u_scene_linearize: Option<glow::UniformLocation>,
    u_transp_linearize: Option<glow::UniformLocation>,
    u_scene_shadow_on: Option<glow::UniformLocation>,
    u_scene_light_mvp: Option<glow::UniformLocation>,
    u_scene_csm_n: Option<glow::UniformLocation>,
    u_scene_shadow: Option<glow::UniformLocation>,
    // Depth (shadow-map) program + its FBO/texture.
    depth_prog: Option<glow::Program>,
    u_depth_mvp: Option<glow::UniformLocation>,
    shadow_fbo: Option<glow::Framebuffer>,
    shadow_tex: Option<glow::Texture>,
    shadow_size: i32,
    tex_bufs: std::collections::HashMap<u64, (glow::VertexArray, glow::Buffer, i32)>,
    /// Uploaded GL images, keyed by `(app texture index, is-sRGB)` — see [`Self::ensure_texture`].
    tex_images: std::collections::HashMap<(usize, bool), glow::Texture>,
    // Shared DYNAMIC buffer for textured FEATURES (walls/floors), whose world-space geometry
    // changes on every recompute — re-uploaded each frame instead of cached per key.
    tex_dyn_vao: Option<glow::VertexArray>,
    tex_dyn_vbo: Option<glow::Buffer>,
    blit_prog: Option<glow::Program>,
    u_tex: Option<glow::UniformLocation>,
    // The composite is where colour management happens (see `crate::color`) and where the direct
    // and ambient buffers are recombined with occlusion.
    u_blit_view: Option<glow::UniformLocation>,
    u_blit_exposure: Option<glow::UniformLocation>,
    u_blit_look: Option<glow::UniformLocation>,
    u_blit_punchy: Option<glow::UniformLocation>,
    u_blit_amb: Option<glow::UniformLocation>,
    u_blit_ao: Option<glow::UniformLocation>,
    u_blit_ao_on: Option<glow::UniformLocation>,
    u_blit_bloom: Option<glow::UniformLocation>,
    u_blit_bloom_k: Option<glow::UniformLocation>,
    u_blit_ssgi: Option<glow::UniformLocation>,
    u_blit_ssgi_k: Option<glow::UniformLocation>,
    u_blit_composed: Option<glow::UniformLocation>,
    blit_vao: Option<glow::VertexArray>,
    blit_vbo: Option<glow::Buffer>,
    // SKY BACKDROP: a full-viewport pass that draws the physical sky behind the model.
    sky_prog: Option<glow::Program>,
    u_sky_inv_vp: Option<glow::UniformLocation>,
    u_sky_cam: Option<glow::UniformLocation>,
    sky_u_bg: SkyUniforms,
    // SSAO: an occlusion pass over the depth texture, then a box blur.
    ssao_prog: Option<glow::Program>,
    u_ssao_depth: Option<glow::UniformLocation>,
    u_ssao_vp: Option<glow::UniformLocation>,
    u_ssao_inv_vp: Option<glow::UniformLocation>,
    u_ssao_cam: Option<glow::UniformLocation>,
    u_ssao_radius: Option<glow::UniformLocation>,
    u_ssao_strength: Option<glow::UniformLocation>,
    blur_prog: Option<glow::Program>,
    u_blur_ao: Option<glow::UniformLocation>,
    /// SSGI: the gather and its RGB blur, plus two HALF-resolution targets they ping-pong through.
    /// Half res because a bounce is a broad, soft term — the detail it would gain at full
    /// resolution is detail the blur exists to remove, so it would be paid for twice.
    ssgi_prog: Option<glow::Program>,
    blur_rgb_prog: Option<glow::Program>,
    ssgi_fbo: [Option<glow::Framebuffer>; 2],
    ssgi_tex: [Option<glow::Texture>; 2],
    ssgi_size: (i32, i32),
    ao_fbo: [Option<glow::Framebuffer>; 2],
    ao_tex: [Option<glow::Texture>; 2],
    fbo: Option<glow::Framebuffer>,
    color: Option<glow::Texture>,
    /// Colour attachment 1 — the diffuse AMBIENT term, kept apart so occlusion can scale it alone.
    ambient: Option<glow::Texture>,
    /// Colour attachment 2 — the surface's DIFFUSE ALBEDO, the one thing no other buffer holds.
    /// Every other target carries light already multiplied by it, inseparably; see
    /// [`ALBEDO_OUT_GLSL`] for why screen-space GI cannot be written without this.
    albedo: Option<glow::Texture>,
    /// Depth is a TEXTURE, not a renderbuffer: SSAO reconstructs world positions by sampling it,
    /// which a renderbuffer cannot do.
    depth: Option<glow::Texture>,
    fbo_w: i32,
    fbo_h: i32,
    /// Anisotropy to request on albedo textures — resolved once from the extension list at init.
    /// 0 means the driver has no anisotropic filtering and we stay on plain trilinear.
    aniso_max: f32,
    /// What the driver actually offers (see [`GlCaps`]) — read once at init.
    pub caps: GlCaps,
    /// Programs the driver rejected at init — see [`Scene3dRenderer::shader_failures`].
    shader_fail: Vec<&'static str>,
    /// What the last frame drew with — see [`FrameGeom`].
    geom: FrameGeom,
}

// Safety: glow handles are integer ids; they're only *used* on the GL thread.
unsafe impl Send for Scene3dRenderer {}
unsafe impl Sync for Scene3dRenderer {}

impl Default for Scene3dRenderer {
    fn default() -> Self {
        Self {
            inited: false,
            scene_prog: None,
            u_mvp: None,
            u_alpha: None,
            scene_vao: None,
            scene_vbo: None,
            static_vao: [None, None],
            static_vbo: [None, None],
            static_ver: [u64::MAX, u64::MAX],
            static_len: [0, 0],
            furn_bufs: std::collections::HashMap::new(),
            transp_prog: None,
            u_transp_mvp: None,
            transp_bufs: std::collections::HashMap::new(),
            tex_prog: None,
            u_tex_mvp: None,
            u_tex_img: None,
            u_tex_cam: None,
            u_tex_reflect: None,
            u_tex_proc: None,
            u_tex_col_a: None,
            u_tex_col_b: None,
            u_tex_pscale: None,
            u_tex_detail: None,
            u_tex_prough: None,
            u_tex_pcontrast: None,
            u_tex_ramp: None,
            u_tex_model: None,
            u_tex_sun_on: None,
            u_tex_sun_dir: None,
            u_tex_sun_col: None,
            u_tex_emission: None,
            env_tex: None,
            env_bg_tex: None,
            env_version: 0,
            bloom_pre_prog: None,
            bloom_down_prog: None,
            bloom_up_prog: None,
            bloom_tex: Vec::new(),
            bloom_size: Vec::new(),
            bloom_fbo: None,
            taa_prog: None,
            taa_fbo: None,
            taa_tex: [None, None],
            taa_size: (0, 0),
            taa_cur: 0,
            taa_max: 0,
            taa_n: 0,
            taa_valid: false,
            taa_key: FrameKey::default(),
            taa_stable: false,
            taa_dbg: String::new(),
            sky_u_tex: SkyUniforms::default(),
            sky_u_scene: SkyUniforms::default(),
            u_tex_hl: None,
            u_tex_hl_k: None,
            u_tex_nrm: None,
            u_tex_rough: None,
            u_tex_metal_map: None,
            u_tex_ao_map: None,
            u_tex_has_nrm: None,
            u_tex_has_rough: None,
            u_tex_has_metal: None,
            u_tex_has_ao: None,
            u_tex_triplanar: None,
            u_tex_tpm: None,
            u_tex_rough_lo: None,
            u_tex_rough_hi: None,
            u_tex_bump: None,
            u_tex_rough_base: None,
            u_tex_coat: None,
            u_tex_coat_rough: None,
            u_tex_sheen: None,
            u_tex_sheen_tint: None,
            u_tex_shadow_on: None,
            u_tex_light_mvp: None,
            u_tex_csm_n: None,
            u_tex_shadow: None,
            u_tex_clay: None,
            u_tex_metallic: None,
            u_tex_ior: None,
            u_scene_linearize: None,
            u_transp_linearize: None,
            u_scene_model: None,
            u_scene_shadow_on: None,
            u_scene_light_mvp: None,
            u_scene_csm_n: None,
            u_scene_shadow: None,
            depth_prog: None,
            u_depth_mvp: None,
            shadow_fbo: None,
            shadow_tex: None,
            shadow_size: SHADOW_MAP_SIZE,
            tex_bufs: std::collections::HashMap::new(),
            tex_images: std::collections::HashMap::new(),
            tex_dyn_vao: None,
            tex_dyn_vbo: None,
            blit_prog: None,
            u_tex: None,
            u_blit_view: None,
            u_blit_exposure: None,
            u_blit_look: None,
            u_blit_punchy: None,
            u_blit_composed: None,
            u_blit_amb: None,
            u_blit_ao: None,
            u_blit_ao_on: None,
            u_blit_bloom: None,
            u_blit_bloom_k: None,
            u_blit_ssgi: None,
            u_blit_ssgi_k: None,
            blit_vao: None,
            blit_vbo: None,
            sky_prog: None,
            u_sky_inv_vp: None,
            u_sky_cam: None,
            sky_u_bg: SkyUniforms::default(),
            ssao_prog: None,
            u_ssao_depth: None,
            u_ssao_vp: None,
            u_ssao_inv_vp: None,
            u_ssao_cam: None,
            u_ssao_radius: None,
            u_ssao_strength: None,
            blur_prog: None,
            u_blur_ao: None,
            ssgi_prog: None,
            blur_rgb_prog: None,
            ssgi_fbo: [None, None],
            ssgi_tex: [None, None],
            ssgi_size: (0, 0),
            ao_fbo: [None, None],
            ao_tex: [None, None],
            fbo: None,
            color: None,
            ambient: None,
            albedo: None,
            depth: None,
            fbo_w: 0,
            fbo_h: 0,
            aniso_max: 0.0,
            caps: GlCaps::default(),
            shader_fail: Vec::new(),
            geom: FrameGeom::default(),
        }
    }
}

/// What the GL context can actually do, read once at init.
///
/// This exists to keep a specific mistake from being made twice: treating `#version 330` — a floor
/// this code chose — as the hardware's ceiling. Techniques ruled out for needing compute shaders or
/// SSBOs (screen-space occlusion by horizon scan, GPU-side shadow paging, tracing against a hi-Z
/// pyramid) are only ruled out if `compute`/`ssbo` come back false here.
#[derive(Clone, Debug, Default)]
pub struct GlCaps {
    pub version: String,
    pub renderer: String,
    pub glsl: String,
    /// GL 4.3 — compute shaders.
    pub compute: bool,
    /// GL 4.3 — shader storage buffer objects.
    pub ssbo: bool,
    /// GL 4.2 — image load/store (read-write textures from a shader).
    pub image_load_store: bool,
    /// The driver LISTS `GL_ARB_compute_shader`, which on a 3.3 core context it will not honour.
    /// Kept only so the gap between "advertised" and "usable" is visible rather than surprising.
    pub advertised_compute: bool,
}

impl GlCaps {
    fn query(gl: &glow::Context) -> Self {
        use glow::HasContext as _;
        let (version, renderer, glsl) = unsafe {
            (
                gl.get_parameter_string(glow::VERSION),
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION),
            )
        };
        // The version string starts "<major>.<minor>" for desktop GL; anything unparseable is
        // treated as the 3.3 floor, which is the safe answer.
        let (mj, mn) = {
            let head: String = version.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            let mut it = head.split('.');
            (
                it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(3),
                it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(3),
            )
        };
        let at_least = |a: u32, b: u32| mj > a || (mj == a && mn >= b);
        let ext = gl.supported_extensions();
        // The CONTEXT version is what binds, not the extension string. NVIDIA advertises
        // `GL_ARB_compute_shader` on this machine even though the context is 3.3 core, and
        // ARB_compute_shader's own spec requires a 4.2 baseline — so believing the extension would
        // mean writing a compute path that cannot be created. Report both, and only claim a
        // feature is USABLE when the version grants it.
        Self {
            compute: at_least(4, 3),
            ssbo: at_least(4, 3),
            image_load_store: at_least(4, 2),
            advertised_compute: ext.contains("GL_ARB_compute_shader"),
            version,
            renderer,
            glsl,
        }
    }
}

impl Scene3dRenderer {
    /// Build the bloom pyramid for a `w × h` viewport, reusing it while the size is unchanged.
    ///
    /// Half resolution at the top and halving down: bloom is a wide, low-frequency spill, so
    /// resolving it at full resolution would cost four times the fill rate for the same blur.
    unsafe fn bloom_targets(&mut self, gl: &glow::Context, w: i32, h: i32) -> bool {
        const LEVELS: usize = 6;
        let want: Vec<(i32, i32)> = (0..LEVELS)
            .map(|i| ((w >> (i + 1)).max(1), (h >> (i + 1)).max(1)))
            .take_while(|(lw, lh)| *lw >= 2 && *lh >= 2)
            .collect();
        if want.is_empty() {
            return false;
        }
        if self.bloom_size == want && self.bloom_tex.len() == want.len() {
            return true;
        }
        for t in self.bloom_tex.drain(..) {
            gl.delete_texture(t);
        }
        for &(lw, lh) in &want {
            let Ok(t) = gl.create_texture() else { return false };
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_image_2d(
                glow::TEXTURE_2D, 0, glow::RGBA16F as i32, lw, lh, 0,
                glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            // CLAMP, never repeat: a bright window at the left edge must not bleed in from the right.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            self.bloom_tex.push(t);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.bloom_size = want;
        true
    }

    /// Run the bloom chain over the finished scene buffer, leaving the result in `bloom_tex[0]`.
    ///
    /// Returns false when it could not run, so the composite skips it rather than sampling a
    /// texture nothing ever wrote.
    unsafe fn bloom_pass(
        &mut self, gl: &glow::Context, w: i32, h: i32, ao_ready: bool,
        color: crate::color::ColorPipeline,
    ) -> bool {
        if color.bloom <= 0.0 || !self.bloom_targets(gl, w, h) {
            return false;
        }
        let (Some(fbo), Some(vao), Some(vbo), Some(pre), Some(down), Some(up), Some(src)) = (
            self.bloom_fbo, self.blit_vao, self.blit_vbo,
            self.bloom_pre_prog, self.bloom_down_prog, self.bloom_up_prog, self.color,
        ) else {
            return false;
        };
        // A fullscreen pair of triangles in NDC; every bloom pass draws exactly this.
        const FULL: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0,  1.0, -1.0, 1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
            -1.0, -1.0, 0.0, 0.0,  1.0, 1.0, 1.0, 1.0,  -1.0, 1.0, 0.0, 1.0,
        ];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&FULL), glow::DYNAMIC_DRAW);
        gl.bind_vertex_array(Some(vao));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);

        // 1 — bright pass, straight into the top of the pyramid.
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.bloom_tex[0]), 0,
        );
        gl.viewport(0, 0, self.bloom_size[0].0, self.bloom_size[0].1);
        gl.use_program(Some(pre));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(src));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, self.ambient);
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, if ao_ready { self.ao_tex[1] } else { None });
        for (n, v) in [("u_tex", 0), ("u_amb", 1), ("u_ao", 2), ("u_ao_on", ao_ready as i32)] {
            if let Some(l) = gl.get_uniform_location(pre, n) {
                gl.uniform_1_i32(Some(&l), v);
            }
        }
        let thr = color.bloom_threshold.max(0.0);
        if let Some(l) = gl.get_uniform_location(pre, "u_threshold") {
            gl.uniform_1_f32(Some(&l), thr);
        }
        if let Some(l) = gl.get_uniform_location(pre, "u_knee") {
            // A quarter of the threshold: wide enough to hide the contour, narrow enough that
            // ordinary lit surfaces still do not bloom.
            gl.uniform_1_f32(Some(&l), (thr * 0.25).max(0.02));
        }
        gl.active_texture(glow::TEXTURE0);
        gl.draw_arrays(glow::TRIANGLES, 0, 6);

        // 2 — downsample to the bottom of the pyramid.
        gl.use_program(Some(down));
        if let Some(l) = gl.get_uniform_location(down, "u_tex") {
            gl.uniform_1_i32(Some(&l), 0);
        }
        for lv in 1..self.bloom_tex.len() {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.bloom_tex[lv]), 0,
            );
            gl.viewport(0, 0, self.bloom_size[lv].0, self.bloom_size[lv].1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bloom_tex[lv - 1]));
            let (sw, sh) = self.bloom_size[lv - 1];
            if let Some(l) = gl.get_uniform_location(down, "u_texel") {
                gl.uniform_2_f32(Some(&l), 1.0 / sw as f32, 1.0 / sh as f32);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }

        // 3 — upsample back up, ADDING at each step. Successive small blurs compose into one wide
        // blur, which is the whole trick: a single Gaussian this wide would need hundreds of taps.
        gl.use_program(Some(up));
        if let Some(l) = gl.get_uniform_location(up, "u_tex") {
            gl.uniform_1_i32(Some(&l), 0);
        }
        gl.enable(glow::BLEND);
        gl.blend_func(glow::ONE, glow::ONE);
        for lv in (0..self.bloom_tex.len() - 1).rev() {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.bloom_tex[lv]), 0,
            );
            gl.viewport(0, 0, self.bloom_size[lv].0, self.bloom_size[lv].1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bloom_tex[lv + 1]));
            let (sw, sh) = self.bloom_size[lv + 1];
            if let Some(l) = gl.get_uniform_location(up, "u_texel") {
                gl.uniform_2_f32(Some(&l), 1.0 / sw as f32, 1.0 / sh as f32);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }

        gl.disable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        for u in [1u32, 2] {
            gl.active_texture(glow::TEXTURE0 + u);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        true
    }

    /// Set the fog uniforms on `prog` and bind the depth buffer it reads.
    ///
    /// The DEPTH unit is 5 — 0–4 are the colour attachment, the ambient, occlusion, bloom and the
    /// accumulation history. Passing `on: false` is how the composite is told the fog is already in
    /// its input: the uniforms still go up, so the shader stays valid, but the branch is skipped.
    unsafe fn set_fog(
        &self, gl: &glow::Context, prog: glow::Program, fog: &crate::env::FogSettings,
        cam: [f32; 3], inv_vp: &[f32; 16], on: bool,
    ) {
        const UNIT: i32 = 5;
        gl.active_texture(glow::TEXTURE0 + UNIT as u32);
        gl.bind_texture(glow::TEXTURE_2D, self.depth);
        gl.active_texture(glow::TEXTURE0);
        let on = on && fog.enabled && fog.density > 0.0 && self.depth.is_some();
        for (n, v) in [("u_fog_depth", UNIT), ("u_fog_on", on as i32)] {
            if let Some(l) = gl.get_uniform_location(prog, n) {
                gl.uniform_1_i32(Some(&l), v);
            }
        }
        if !on {
            return; // nothing else is read; skip the uploads
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_fog_inv_vp") {
            gl.uniform_matrix_4_f32_slice(Some(&l), false, inv_vp);
        }
        for (n, v) in [("u_fog_cam", cam), ("u_fog_col", fog.color)] {
            if let Some(l) = gl.get_uniform_location(prog, n) {
                gl.uniform_3_f32(Some(&l), v[0], v[1], v[2]);
            }
        }
        for (n, v) in [
            ("u_fog_density", fog.density.max(0.0)),
            ("u_fog_base", fog.base_z),
            ("u_fog_falloff", fog.falloff.max(0.0)),
        ] {
            if let Some(l) = gl.get_uniform_location(prog, n) {
                gl.uniform_1_f32(Some(&l), v);
            }
        }
    }

    /// Ask for temporal accumulation: refine a STILL frame over up to `samples` sub-pixel jitters.
    ///
    /// `samples <= 1` turns it off and frees the buffers. Call this every frame — the renderer is
    /// shared with the SIMLUX lighting view, which must never accumulate (its vertex colours are a
    /// false-colour measurement, and averaging jittered samples of a scale would blur the reading).
    /// The buffers are deliberately NOT freed here, only on a viewport resize (as bloom's pyramid
    /// is). The lighting view calls this with 0 every frame it paints; freeing on the way down
    /// would mean allocating two full-screen RGBA16F targets per frame whenever both views are
    /// on screen at once — a stutter, to reclaim memory that is about to be needed again.
    pub fn set_taa(&mut self, _gl: &glow::Context, samples: u32) {
        let want = if samples <= 1 { 0 } else { samples.min(64) };
        if want != self.taa_max {
            self.taa_max = want;
            self.taa_n = 0;
            self.taa_valid = false;
        }
    }

    /// Shaders the driver refused, by name. Empty is the healthy case.
    ///
    /// A failed program is not a crash — the feature it drove stops drawing and everything else
    /// carries on, which is right, but it also means the symptom reaches the user as "part of the
    /// picture is wrong" with nothing to point at. This puts the names somewhere a session dump
    /// can report them.
    pub fn shader_failures(&self) -> &[&'static str] {
        &self.shader_fail
    }

    /// What the last frame drew with — see [`FrameGeom`].
    pub fn last_geom(&self) -> FrameGeom {
        self.geom
    }

    /// Record the frame's geometry for the session dump. Cheap: one `check_framebuffer_status`.
    unsafe fn note_geom(
        &mut self, gl: &glow::Context, vp: (i32, i32, i32, i32), screen: (i32, i32),
        env: &crate::env::EnvRender, cascades: usize,
    ) {
        let complete = self.fbo.is_some_and(|f| {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
            let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            ok
        });
        self.geom = FrameGeom {
            vp,
            screen,
            fbo: (self.fbo_w, self.fbo_h),
            taa: self.taa_size,
            taa_valid: self.taa_valid,
            taa_n: self.taa_n,
            taa_max: self.taa_max,
            bloom0: self.bloom_size.first().copied().unwrap_or((0, 0)),
            fbo_complete: complete,
            env: env.hdri.is_some(),
            cascades: cascades as u32,
        };
    }

    /// True while the picture is still being refined — the caller should keep asking for repaints.
    ///
    /// Requires the frame to have HELD STILL for at least one repaint (`taa_stable`); see that
    /// field for why. Read one frame late (the paint callback runs after the UI code that checks
    /// it), which is harmless: at worst it costs one extra repaint at the end of a convergence.
    pub fn taa_converging(&self) -> bool {
        self.taa_max > 0 && self.taa_stable && self.taa_n < self.taa_max
    }

    /// How far the refinement has got, as `(samples so far, target)` — for a progress hint.
    pub fn taa_progress(&self) -> (u32, u32) {
        (self.taa_n, self.taa_max)
    }

    /// Allocate (or resize) the two accumulation buffers. Returns false if the driver refuses.
    unsafe fn taa_targets(&mut self, gl: &glow::Context, w: i32, h: i32) -> bool {
        if self.taa_tex[0].is_some() && self.taa_size == (w, h) {
            return self.taa_fbo.is_some();
        }
        for t in self.taa_tex.iter_mut().filter_map(Option::take) {
            gl.delete_texture(t);
        }
        self.taa_valid = false; // the fresh buffers hold nothing until a resolve writes one
        for i in 0..2 {
            let Ok(t) = gl.create_texture() else { return false };
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            // RGBA16F for the same reason every other buffer here is: this holds scene-referred
            // light, which routinely exceeds 1.0. Accumulating in 8 bits would clip the highlights
            // the view transform exists to roll off, and quantise the average into banding.
            gl.tex_image_2d(
                glow::TEXTURE_2D, 0, glow::RGBA16F as i32, w, h, 0,
                glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None),
            );
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            self.taa_tex[i] = Some(t);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.taa_size = (w, h);
        if self.taa_fbo.is_none() {
            self.taa_fbo = gl.create_framebuffer().ok();
        }
        self.taa_fbo.is_some()
    }

    /// Compose this frame's light and average it into the history, leaving the running mean in
    /// `taa_tex[taa_cur]`. Returns the texture the composite should read, or `None` on failure —
    /// in which case the caller falls back to the ordinary single-sample path.
    unsafe fn taa_resolve(
        &mut self, gl: &glow::Context, w: i32, h: i32, ao_ready: bool, bloom_ready: bool,
        ssgi_ready: bool, color: crate::color::ColorPipeline, fog: &crate::env::FogSettings,
        cam: [f32; 3], inv_vp: &[f32; 16], gi: &crate::env::GiSettings,
    ) -> Option<glow::Texture> {
        if !self.taa_targets(gl, w, h) {
            return None;
        }
        let (Some(prog), Some(fbo), Some(vao), Some(vbo), Some(src)) =
            (self.taa_prog, self.taa_fbo, self.blit_vao, self.blit_vbo, self.color)
        else {
            return None;
        };
        let prev = self.taa_cur;
        let next = 1 - prev;
        let (Some(hist), Some(dst)) = (self.taa_tex[prev], self.taa_tex[next]) else {
            return None;
        };
        const FULL: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0,  1.0, -1.0, 1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
            -1.0, -1.0, 0.0, 0.0,  1.0, 1.0, 1.0, 1.0,  -1.0, 1.0, 0.0, 1.0,
        ];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&FULL), glow::DYNAMIC_DRAW);
        gl.bind_vertex_array(Some(vao));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(dst), 0,
        );
        gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        gl.viewport(0, 0, w, h);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.use_program(Some(prog));
        for (unit, tex) in [
            (0, Some(src)),
            (1, self.ambient),
            (2, if ao_ready { self.ao_tex[1] } else { None }),
            (3, bloom_ready.then(|| self.bloom_tex[0])),
            (4, Some(hist)),
            (6, ssgi_ready.then_some(self.ssgi_tex[1]).flatten()),
        ] {
            gl.active_texture(glow::TEXTURE0 + unit as u32);
            gl.bind_texture(glow::TEXTURE_2D, tex);
        }
        for (n, v) in [
            ("u_tex", 0), ("u_amb", 1), ("u_ao", 2), ("u_bloom", 3), ("u_hist", 4), ("u_ssgi", 6),
            ("u_ao_on", ao_ready as i32),
        ] {
            if let Some(l) = gl.get_uniform_location(prog, n) {
                gl.uniform_1_i32(Some(&l), v);
            }
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_bloom_k") {
            gl.uniform_1_f32(Some(&l), if bloom_ready { color.bloom } else { 0.0 });
        }
        // 1, not the strength: the gather has already applied it. Multiplying here as well would
        // square it, so the slider would behave like a curve and "2" would mean four times.
        if let Some(l) = gl.get_uniform_location(prog, "u_ssgi_k") {
            gl.uniform_1_f32(Some(&l), if ssgi_ready { 1.0 } else { 0.0 });
        }
        // 1/(n+1): the running result stays the exact unweighted mean of every sample so far,
        // rather than the exponential fade a fixed blend factor would give (which never quite
        // converges and keeps a trace of the first frame forever).
        if let Some(l) = gl.get_uniform_location(prog, "u_blend") {
            gl.uniform_1_f32(Some(&l), 1.0 / (self.taa_n + 1) as f32);
        }
        self.set_fog(gl, prog, fog, cam, inv_vp, true);
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        for unit in 0..6u32 {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        self.taa_cur = next;
        self.taa_n += 1;
        self.taa_valid = true; // `dst` now holds a real image — the re-present path may use it
        Some(dst)
    }

    /// The one place scene-referred light becomes display pixels: draw `src` into the panel rect
    /// through the view transform. `composed` says `src` already has ambient/occlusion/bloom in it.
    unsafe fn composite(
        &mut self, gl: &glow::Context, quad: &[f32; 24], src: glow::Texture, composed: bool,
        ao_ready: bool, bloom_ready: bool, ssgi_ready: bool, color: crate::color::ColorPipeline,
        fog: &crate::env::FogSettings, cam: [f32; 3], inv_vp: &[f32; 16], rect: (i32, i32, i32, i32),
    ) {
        let (Some(prog), Some(vao), Some(vbo)) = (self.blit_prog, self.blit_vao, self.blit_vbo)
        else {
            return;
        };
        // Scissor to OUR OWN viewport rect rather than trusting whatever egui left set.
        //
        // egui scissors a callback to its CLIP rect, which is the enclosing panel — not the
        // smaller rect the 3D view was allocated inside it. So a quad that is even slightly wrong,
        // or a panel with anything else in it, paints the scene over its neighbours. The rect is
        // right here in the arguments; there is no reason to infer it from GL state.
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(rect.0, rect.1, rect.2.max(0), rect.3.max(0));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(quad), glow::DYNAMIC_DRAW);
        gl.use_program(Some(prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(src));
        if let Some(loc) = &self.u_tex { gl.uniform_1_i32(Some(loc), 0); }
        // Attachment 1 (the ambient) and the blurred occlusion, on their own units.
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, self.ambient);
        if let Some(loc) = &self.u_blit_amb { gl.uniform_1_i32(Some(loc), 1); }
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, if ao_ready { self.ao_tex[1] } else { None });
        if let Some(loc) = &self.u_blit_ao { gl.uniform_1_i32(Some(loc), 2); }
        if let Some(loc) = &self.u_blit_ao_on { gl.uniform_1_i32(Some(loc), (ao_ready && !composed) as i32); }
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, bloom_ready.then(|| self.bloom_tex[0]));
        if let Some(loc) = &self.u_blit_bloom { gl.uniform_1_i32(Some(loc), 3); }
        if let Some(loc) = &self.u_blit_bloom_k {
            gl.uniform_1_f32(Some(loc), if bloom_ready && !composed { color.bloom } else { 0.0 });
        }
        gl.active_texture(glow::TEXTURE0 + 6);
        gl.bind_texture(glow::TEXTURE_2D, ssgi_ready.then_some(self.ssgi_tex[1]).flatten());
        if let Some(loc) = &self.u_blit_ssgi { gl.uniform_1_i32(Some(loc), 6); }
        // 1, not the strength — the gather already applied it. And zero when the input is already
        // composed, so an accumulated buffer's bounce is never counted a second time.
        if let Some(loc) = &self.u_blit_ssgi_k {
            gl.uniform_1_f32(Some(loc), (ssgi_ready && !composed) as i32 as f32);
        }
        if let Some(loc) = &self.u_blit_composed { gl.uniform_1_i32(Some(loc), composed as i32); }
        // `!composed`: an accumulated buffer already has the fog folded in.
        self.set_fog(gl, prog, fog, cam, inv_vp, !composed);
        gl.active_texture(glow::TEXTURE0);
        if let Some(loc) = &self.u_blit_view { gl.uniform_1_i32(Some(loc), color.view.id()); }
        if let Some(loc) = &self.u_blit_exposure { gl.uniform_1_f32(Some(loc), color.exposure); }
        if let Some(loc) = &self.u_blit_look { gl.uniform_1_f32(Some(loc), color.look); }
        if let Some(loc) = &self.u_blit_punchy { gl.uniform_1_f32(Some(loc), color.punchy); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        for unit in 0..7u32 {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
    }

    /// Upload an HDR environment, or drop the one that is loaded.
    ///
    /// The chain arrives as a real mip chain (each level exactly half the one above), so it goes up
    /// as ONE `GL_TEXTURE_2D` with explicit levels and `LINEAR_MIPMAP_LINEAR` filtering. That is
    /// what makes the shader side a single `textureLod(u_env, uv, roughness * 5)` — the hardware
    /// interpolates between two roughnesses for nothing, and there is no second sampler to bind.
    ///
    /// Wrapping is REPEAT across longitude and CLAMP down latitude: an equirect map is continuous
    /// where its left edge meets its right, and is not continuous over the poles.
    pub fn set_environment(&mut self, gl: &glow::Context, up: Option<EnvUpload<'_>>) {
        unsafe {
            match up {
                None => {
                    for t in [self.env_tex.take(), self.env_bg_tex.take()].into_iter().flatten() {
                        gl.delete_texture(t);
                    }
                    self.env_version = 0;
                }
                Some(up) => {
                    if self.env_version == up.version && self.env_tex.is_some() {
                        return; // same environment; the pixels are already on the GPU
                    }
                    for t in [self.env_tex.take(), self.env_bg_tex.take()].into_iter().flatten() {
                        gl.delete_texture(t);
                    }
                    // The BACKDROP copy: full resolution, its own ordinary mip chain (so a distant
                    // grazing view still filters properly rather than shimmering).
                    if let Ok(bg) = gl.create_texture() {
                        let flat: Vec<f32> = up.source.px.iter().flatten().map(half_safe).collect();
                        gl.bind_texture(glow::TEXTURE_2D, Some(bg));
                        gl.tex_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            glow::RGB16F as i32,
                            up.source.w as i32,
                            up.source.h as i32,
                            0,
                            glow::RGB,
                            glow::FLOAT,
                            glow::PixelUnpackData::Slice(Some(bytemuck_cast(&flat))),
                        );
                        gl.generate_mipmap(glow::TEXTURE_2D);
                        gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_MIN_FILTER,
                            glow::LINEAR_MIPMAP_LINEAR as i32,
                        );
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                        gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_WRAP_T,
                            glow::CLAMP_TO_EDGE as i32,
                        );
                        self.env_bg_tex = Some(bg);
                    }
                    let Ok(tex) = gl.create_texture() else { return };
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                    for (level, mip) in up.chain.iter().enumerate() {
                        let flat: Vec<f32> = mip.px.iter().flatten().map(half_safe).collect();
                        gl.tex_image_2d(
                            glow::TEXTURE_2D,
                            level as i32,
                            glow::RGB16F as i32,
                            mip.w as i32,
                            mip.h as i32,
                            0,
                            glow::RGB,
                            glow::FLOAT,
                            glow::PixelUnpackData::Slice(Some(bytemuck_cast(&flat))),
                        );
                    }
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MAX_LEVEL,
                        up.chain.len() as i32 - 1,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MIN_FILTER,
                        glow::LINEAR_MIPMAP_LINEAR as i32,
                    );
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_T,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    self.env_tex = Some(tex);
                    self.env_version = up.version;
                }
            }
        }
    }

    fn ensure_init(&mut self, gl: &glow::Context) {
        if self.inited {
            return;
        }
        unsafe {
            // What the DRIVER actually gave us. Every shader here is `#version 330`, but that is
            // our choice, not the context's: eframe builds the context with
            // `ContextAttributesBuilder::new()` and no version, and glutin's default asks for the
            // highest CORE version the driver supports — 4.6 on any current desktop GPU. Compute
            // shaders (4.3), SSBOs (4.3) and image load/store (4.2) are therefore probably already
            // available, and several rendering techniques we have written off as out of reach are
            // only out of reach of the floor we picked. Record it so that is a fact rather than an
            // assumption; `caps` is what a higher-tier path would branch on.
            self.caps = GlCaps::query(gl);
            let line = format!(
                "[gl] {} | {} | GLSL {} | usable: compute={} ssbo={} image_ls={} (advertised compute={})",
                self.caps.version, self.caps.renderer, self.caps.glsl,
                self.caps.compute, self.caps.ssbo, self.caps.image_load_store,
                self.caps.advertised_compute,
            );
            eprintln!("{line}");
            // Also to a FILE beside the executable. A console line is easy to miss, scroll away or
            // never see at all depending on how the app was launched, and this answer decides how
            // several rendering features get built — it should not depend on catching it live.
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let _ = std::fs::write(dir.join("gl_caps.txt"), format!("{line}\n"));
                }
            }
            // Anisotropic filtering is an extension on GL 3.3 — resolve it once, by name.
            if gl.supported_extensions().contains("GL_EXT_texture_filter_anisotropic")
                || gl.supported_extensions().contains("GL_ARB_texture_filter_anisotropic")
            {
                self.aniso_max = gl.get_parameter_f32(0x84FF).clamp(1.0, 8.0); // MAX_TEXTURE_MAX_ANISOTROPY
            }
            // --- scene program + VAO (position + colour + ambient share, interleaved) ---
            let scene_fs = assemble_scene_fs();
            if let Some(scene_prog) = compile(gl, "scene", SCENE_VS, &scene_fs) {
                self.u_mvp = gl.get_uniform_location(scene_prog, "u_mvp");
                self.u_alpha = gl.get_uniform_location(scene_prog, "u_alpha");
                self.u_scene_linearize = gl.get_uniform_location(scene_prog, "u_linearize");
                self.u_scene_model = gl.get_uniform_location(scene_prog, "u_model");
                self.u_scene_shadow_on = gl.get_uniform_location(scene_prog, "u_shadow_on");
                self.u_scene_light_mvp = gl.get_uniform_location(scene_prog, "u_light_mvp");
                self.u_scene_csm_n = gl.get_uniform_location(scene_prog, "u_csm_n");
                self.u_scene_shadow = gl.get_uniform_location(scene_prog, "u_shadow");
                self.sky_u_scene = SkyUniforms::locate(gl, scene_prog);
                self.scene_prog = Some(scene_prog);
            }
            // --- depth (shadow map) program ---
            if let Some(depth_prog) = compile(gl, "depth", DEPTH_VS, DEPTH_FS) {
                self.u_depth_mvp = gl.get_uniform_location(depth_prog, "u_depth_mvp");
                self.depth_prog = Some(depth_prog);
            }
            let svbo = gl.create_buffer().unwrap();
            let svao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(svao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(svbo));
            v3_attribs(gl);

            // --- two STATIC scene VAOs/VBOs (same pos+colour+ambient layout) ---
            for i in 0..2 {
                let vbo = gl.create_buffer().unwrap();
                let vao = gl.create_vertex_array().unwrap();
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                v3_attribs(gl);
                self.static_vao[i] = Some(vao);
                self.static_vbo[i] = Some(vbo);
            }

            // --- transparent furniture program (pos+colour+opacity); VAOs built lazily ---
            let transp_fs = assemble_transp_fs();
            if let Some(transp_prog) = compile(gl, "transparent", TRANSP_VS, &transp_fs) {
                self.u_transp_mvp = gl.get_uniform_location(transp_prog, "u_mvp");
                self.u_transp_linearize = gl.get_uniform_location(transp_prog, "u_linearize");
                self.transp_prog = Some(transp_prog);
            }

            // --- textured program (pos+uv+shade); per-mesh VAOs are built lazily ---
            let tex_fs = assemble_tex_fs();
            if let Some(tex_prog) = compile(gl, "textured", TEX_VS, &tex_fs) {
                self.u_tex_mvp = gl.get_uniform_location(tex_prog, "u_mvp");
                self.u_tex_img = gl.get_uniform_location(tex_prog, "u_img");
                self.u_tex_cam = gl.get_uniform_location(tex_prog, "u_cam");
                self.u_tex_reflect = gl.get_uniform_location(tex_prog, "u_reflect");
                self.u_tex_proc = gl.get_uniform_location(tex_prog, "u_proc");
                self.u_tex_col_a = gl.get_uniform_location(tex_prog, "u_col_a");
                self.u_tex_col_b = gl.get_uniform_location(tex_prog, "u_col_b");
                self.u_tex_pscale = gl.get_uniform_location(tex_prog, "u_pscale");
                self.u_tex_detail = gl.get_uniform_location(tex_prog, "u_detail");
                self.u_tex_prough = gl.get_uniform_location(tex_prog, "u_prough");
                self.u_tex_pcontrast = gl.get_uniform_location(tex_prog, "u_pcontrast");
                self.u_tex_ramp = gl.get_uniform_location(tex_prog, "u_ramp");
                self.u_tex_model = gl.get_uniform_location(tex_prog, "u_model");
                self.u_tex_sun_on = gl.get_uniform_location(tex_prog, "u_sun_on");
                self.u_tex_sun_dir = gl.get_uniform_location(tex_prog, "u_sun_dir");
                self.u_tex_sun_col = gl.get_uniform_location(tex_prog, "u_sun_col");
                self.u_tex_emission = gl.get_uniform_location(tex_prog, "u_emission");
                self.sky_u_tex = SkyUniforms::locate(gl, tex_prog);
                self.u_tex_hl = gl.get_uniform_location(tex_prog, "u_hl");
                self.u_tex_hl_k = gl.get_uniform_location(tex_prog, "u_hl_k");
                self.u_tex_nrm = gl.get_uniform_location(tex_prog, "u_nrm");
                self.u_tex_rough = gl.get_uniform_location(tex_prog, "u_rough");
                self.u_tex_metal_map = gl.get_uniform_location(tex_prog, "u_metal");
                self.u_tex_ao_map = gl.get_uniform_location(tex_prog, "u_aomap");
                self.u_tex_has_nrm = gl.get_uniform_location(tex_prog, "u_has_nrm");
                self.u_tex_has_rough = gl.get_uniform_location(tex_prog, "u_has_rough");
                self.u_tex_has_metal = gl.get_uniform_location(tex_prog, "u_has_metal");
                self.u_tex_has_ao = gl.get_uniform_location(tex_prog, "u_has_ao");
                self.u_tex_triplanar = gl.get_uniform_location(tex_prog, "u_triplanar");
                self.u_tex_tpm = gl.get_uniform_location(tex_prog, "u_tpm");
                self.u_tex_rough_lo = gl.get_uniform_location(tex_prog, "u_rough_lo");
                self.u_tex_rough_hi = gl.get_uniform_location(tex_prog, "u_rough_hi");
                self.u_tex_bump = gl.get_uniform_location(tex_prog, "u_bump");
                self.u_tex_rough_base = gl.get_uniform_location(tex_prog, "u_rough_base");
                self.u_tex_coat = gl.get_uniform_location(tex_prog, "u_coat");
                self.u_tex_coat_rough = gl.get_uniform_location(tex_prog, "u_coat_rough");
                self.u_tex_sheen = gl.get_uniform_location(tex_prog, "u_sheen");
                self.u_tex_sheen_tint = gl.get_uniform_location(tex_prog, "u_sheen_tint");
                self.u_tex_shadow_on = gl.get_uniform_location(tex_prog, "u_shadow_on");
                self.u_tex_light_mvp = gl.get_uniform_location(tex_prog, "u_light_mvp");
                self.u_tex_csm_n = gl.get_uniform_location(tex_prog, "u_csm_n");
                self.u_tex_shadow = gl.get_uniform_location(tex_prog, "u_shadow");
                self.u_tex_clay = gl.get_uniform_location(tex_prog, "u_clay");
                self.u_tex_metallic = gl.get_uniform_location(tex_prog, "u_metallic");
                self.u_tex_ior = gl.get_uniform_location(tex_prog, "u_ior");
                self.tex_prog = Some(tex_prog);
            }
            // Shared DYNAMIC textured buffer (feature surfaces re-uploaded each frame).
            {
                let vbo = gl.create_buffer().unwrap();
                let vao = gl.create_vertex_array().unwrap();
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                let stride = size_of::<TexVtx>() as i32;
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 12);
                gl.enable_vertex_attrib_array(2);
                gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 20);
                gl.enable_vertex_attrib_array(3);
                gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 24);
                self.tex_dyn_vao = Some(vao);
                self.tex_dyn_vbo = Some(vbo);
            }

            // --- blit program + a dynamic 6-vertex quad (pos.xy, uv) ---
            // Bloom's three passes. They share the blit's fullscreen vertex shader.
            // A ceiling far inside half-float range, so the pyramid's own sums cannot reach it
            // either: even four taps of the maximum leave headroom before 65504.
            let bloom_pre = BLOOM_PRE_FS.replace("BLOOM_CEILING", "4000.0");
            self.bloom_pre_prog = compile(gl, "bloom-prefilter", BLIT_VS, &bloom_pre);
            self.bloom_down_prog = compile(gl, "bloom-down", BLIT_VS, BLOOM_DOWN_FS);
            self.bloom_up_prog = compile(gl, "bloom-up", BLIT_VS, BLOOM_UP_FS);
            self.bloom_fbo = gl.create_framebuffer().ok();

            let blit_fs = BLIT_FS
                .replace("FOG_GLSL", &fog_glsl())
                .replace("VIEW_GLSL", crate::color::VIEW_GLSL);
            if let Some(blit_prog) = compile(gl, "composite", BLIT_VS, &blit_fs) {
                self.u_tex = gl.get_uniform_location(blit_prog, "u_tex");
                self.u_blit_view = gl.get_uniform_location(blit_prog, "u_view");
                self.u_blit_exposure = gl.get_uniform_location(blit_prog, "u_exposure");
                self.u_blit_look = gl.get_uniform_location(blit_prog, "u_look");
                self.u_blit_punchy = gl.get_uniform_location(blit_prog, "u_punchy");
                self.u_blit_amb = gl.get_uniform_location(blit_prog, "u_amb");
                self.u_blit_ao = gl.get_uniform_location(blit_prog, "u_ao");
                self.u_blit_ao_on = gl.get_uniform_location(blit_prog, "u_ao_on");
                self.u_blit_bloom = gl.get_uniform_location(blit_prog, "u_bloom");
                self.u_blit_bloom_k = gl.get_uniform_location(blit_prog, "u_bloom_k");
                self.u_blit_ssgi = gl.get_uniform_location(blit_prog, "u_ssgi");
                self.u_blit_ssgi_k = gl.get_uniform_location(blit_prog, "u_ssgi_k");
                self.u_blit_composed = gl.get_uniform_location(blit_prog, "u_composed");
                self.blit_prog = Some(blit_prog);
            }
            // Temporal accumulation shares the blit's fullscreen vertex shader too.
            self.taa_prog = compile(gl, "taa-resolve", BLIT_VS, &TAA_FS.replace("FOG_GLSL", &fog_glsl()));
            self.taa_fbo = gl.create_framebuffer().ok();

            // --- sky backdrop, SSAO and its blur: all full-viewport passes sharing BLIT_VS ---
            let sky_fs = assemble_sky_fs();
            if let Some(p) = compile(gl, "sky", BLIT_VS, &sky_fs) {
                self.u_sky_inv_vp = gl.get_uniform_location(p, "u_inv_vp");
                self.u_sky_cam = gl.get_uniform_location(p, "u_cam");
                self.sky_u_bg = SkyUniforms::locate(gl, p);
                self.sky_prog = Some(p);
            }
            if let Some(p) = compile(gl, "ssao", BLIT_VS, SSAO_FS) {
                self.u_ssao_depth = gl.get_uniform_location(p, "u_depth");
                self.u_ssao_vp = gl.get_uniform_location(p, "u_vp");
                self.u_ssao_inv_vp = gl.get_uniform_location(p, "u_inv_vp");
                self.u_ssao_cam = gl.get_uniform_location(p, "u_cam");
                self.u_ssao_radius = gl.get_uniform_location(p, "u_radius");
                self.u_ssao_strength = gl.get_uniform_location(p, "u_strength");
                self.ssao_prog = Some(p);
            }
            if let Some(p) = compile(gl, "ao blur", BLIT_VS, BLUR_FS) {
                self.u_blur_ao = gl.get_uniform_location(p, "u_ao");
                self.blur_prog = Some(p);
            }
            self.ssgi_prog = compile(gl, "ssgi", BLIT_VS, SSGI_FS);
            self.blur_rgb_prog = compile(gl, "rgb blur", BLIT_VS, BLUR_RGB_FS);

            let bvbo = gl.create_buffer().unwrap();
            let bvao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(bvao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(bvbo));
            let bstride = (4 * size_of::<f32>()) as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, bstride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, bstride, 8);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            self.scene_vao = Some(svao);
            self.scene_vbo = Some(svbo);
            self.blit_vao = Some(bvao);
            self.blit_vbo = Some(bvbo);
            // Which programs the driver refused. `compile` already logs the GLSL error to stderr,
            // but a user reporting "part of the picture is wrong" has no stderr — this puts the
            // names where a session dump can carry them back.
            self.shader_fail = [
                ("scene", self.scene_prog.is_none()),
                ("textured", self.tex_prog.is_none()),
                ("transparent", self.transp_prog.is_none()),
                ("composite", self.blit_prog.is_none()),
                ("depth", self.depth_prog.is_none()),
                ("sky", self.sky_prog.is_none()),
                ("ssao", self.ssao_prog.is_none()),
                ("ao-blur", self.blur_prog.is_none()),
                ("bloom-prefilter", self.bloom_pre_prog.is_none()),
                ("bloom-down", self.bloom_down_prog.is_none()),
                ("bloom-up", self.bloom_up_prog.is_none()),
                ("taa-resolve", self.taa_prog.is_none()),
                ("ssgi", self.ssgi_prog.is_none()),
                ("rgb-blur", self.blur_rgb_prog.is_none()),
            ]
            .into_iter()
            .filter_map(|(n, failed)| failed.then_some(n))
            .collect();
            self.inited = true;
        }
    }

    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: i32, h: i32) {
        if self.fbo.is_some() && self.fbo_w == w && self.fbo_h == h {
            return;
        }
        // Tear down any previous attachments.
        if let Some(f) = self.fbo.take() {
            gl.delete_framebuffer(f);
        }
        for t in [self.color.take(), self.ambient.take(), self.albedo.take(), self.depth.take()]
            .into_iter()
            .flatten()
        {
            gl.delete_texture(t);
        }
        // The accumulation buffers are viewport-sized too, and their content is meaningless at a
        // new size. `taa_targets` re-creates them on demand.
        for t in self.taa_tex.iter_mut().filter_map(Option::take) {
            gl.delete_texture(t);
        }
        self.taa_n = 0;
        self.taa_valid = false;
        for i in 0..2 {
            if let Some(f) = self.ao_fbo[i].take() {
                gl.delete_framebuffer(f);
            }
            if let Some(t) = self.ao_tex[i].take() {
                gl.delete_texture(t);
            }
        }

        // A small helper: every target here wants the same clamped, unfiltered-neighbourhood setup.
        let make_tex = |internal: u32, format: u32, ty: u32, filter: u32| -> Option<glow::Texture> {
            let t = gl.create_texture().ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, w, h, 0, format, ty, glow::PixelUnpackData::Slice(None));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            Some(t)
        };

        // RGBA16F, not RGBA8: every pass writes SCENE-REFERRED linear light, which routinely
        // exceeds 1.0 (a sunlit wall, a lamp lens). In 8 bits those values were clipped before the
        // view transform ever saw them, so highlights had nothing left to roll off.
        //
        // TWO of them. Attachment 0 carries light that is already placed — direct sun, specular,
        // emission — and attachment 1 the diffuse ambient. They are recombined at the composite
        // with occlusion applied to the second only.
        let color = make_tex(glow::RGBA16F, glow::RGBA, glow::FLOAT, glow::LINEAR);
        let ambient = make_tex(glow::RGBA16F, glow::RGBA, glow::FLOAT, glow::LINEAR);
        // Attachment 2, the ALBEDO: what each surface would BOUNCE. RGBA8 rather than RGBA16F
        // because an albedo is a reflectance — it lives in 0..1 by definition and cannot blow past
        // the range the way radiance does, so the extra eight bits per channel would buy nothing
        // but bandwidth on a full-screen target written by every draw.
        let albedo = make_tex(glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE, glow::LINEAR);
        // Depth as a TEXTURE: SSAO reconstructs world position by sampling it, which is impossible
        // with the renderbuffer this used to be. NEAREST — interpolating depth across a silhouette
        // would invent surfaces that are not there.
        let depth = make_tex(glow::DEPTH_COMPONENT24, glow::DEPTH_COMPONENT, glow::FLOAT, glow::NEAREST);

        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, color, 0);
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT1, glow::TEXTURE_2D, ambient, 0);
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT2, glow::TEXTURE_2D, albedo, 0);
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, depth, 0);
        gl.draw_buffers(&[
            glow::COLOR_ATTACHMENT0,
            glow::COLOR_ATTACHMENT1,
            glow::COLOR_ATTACHMENT2,
        ]);

        // Two single-channel AO targets: one for the raw occlusion, one for its blur. LINEAR so the
        // composite can sample them at whatever resolution they end up.
        for i in 0..2 {
            let t = make_tex(glow::R8, glow::RED, glow::UNSIGNED_BYTE, glow::LINEAR);
            let f = gl.create_framebuffer().ok();
            if let (Some(t), Some(f)) = (t, f) {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
                gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                self.ao_fbo[i] = Some(f);
                self.ao_tex[i] = Some(t);
            }
        }

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);

        self.fbo = Some(fbo);
        self.color = color;
        self.ambient = ambient;
        self.albedo = albedo;
        self.depth = depth;
        self.fbo_w = w;
        self.fbo_h = h;
    }

    /// Run the occlusion pass over the depth texture and blur it, leaving the result in
    /// `ao_tex[1]`. Returns `false` if anything it needs is missing, in which case the composite
    /// falls back to no occlusion rather than sampling a stale or absent buffer.
    unsafe fn run_ssao(&mut self, gl: &glow::Context, ao: &crate::env::AoSettings, mvp: &[f32; 16], cam: [f32; 3], w: i32, h: i32) -> bool {
        let (Some(prog), Some(blur), Some(depth)) = (self.ssao_prog, self.blur_prog, self.depth) else { return false };
        let (Some(f0), Some(f1), Some(t0)) = (self.ao_fbo[0], self.ao_fbo[1], self.ao_tex[0]) else { return false };
        let (Some(vao), Some(vbo)) = (self.blit_vao, self.blit_vbo) else { return false };
        let inv = Mat4::from_cols_array(mvp).inverse().to_cols_array();

        // A full-NDC quad; the same buffer the composite uses, re-uploaded.
        const FULL: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0,  1.0, -1.0, 1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
            -1.0, -1.0, 0.0, 0.0,  1.0,  1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0,
        ];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&FULL), glow::DYNAMIC_DRAW);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.viewport(0, 0, w, h);

        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f0));
        gl.use_program(Some(prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(depth));
        if let Some(l) = &self.u_ssao_depth { gl.uniform_1_i32(Some(l), 0); }
        if let Some(l) = &self.u_ssao_vp { gl.uniform_matrix_4_f32_slice(Some(l), false, mvp); }
        if let Some(l) = &self.u_ssao_inv_vp { gl.uniform_matrix_4_f32_slice(Some(l), false, &inv); }
        if let Some(l) = &self.u_ssao_cam { gl.uniform_3_f32(Some(l), cam[0], cam[1], cam[2]); }
        if let Some(l) = &self.u_ssao_radius { gl.uniform_1_f32(Some(l), ao.radius.max(0.01)); }
        if let Some(l) = &self.u_ssao_strength { gl.uniform_1_f32(Some(l), ao.strength.clamp(0.0, 2.0)); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 6);

        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f1));
        gl.use_program(Some(blur));
        gl.bind_texture(glow::TEXTURE_2D, Some(t0));
        if let Some(l) = &self.u_blur_ao { gl.uniform_1_i32(Some(l), 0); }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        true
    }

    /// Gather one bounce of light between visible surfaces, leaving the result in `ssgi_tex[1]`.
    ///
    /// Returns false when it could not run, so the composite skips it rather than sampling a
    /// texture nothing ever wrote.
    unsafe fn run_ssgi(
        &mut self, gl: &glow::Context, gi: &crate::env::GiSettings, ao_ready: bool,
        mvp: &[f32; 16], inv_vp: &[f32; 16], cam: [f32; 3], w: i32, h: i32,
    ) -> bool {
        if !gi.enabled || gi.strength <= 0.0 {
            return false;
        }
        // Half resolution. A bounce is broad and soft, and the blur below removes exactly the
        // detail full resolution would buy — so it would be paid for twice.
        let (gw, gh) = ((w / 2).max(1), (h / 2).max(1));
        if self.ssgi_tex[0].is_none() || self.ssgi_size != (gw, gh) {
            for f in self.ssgi_fbo.iter_mut().filter_map(Option::take) {
                gl.delete_framebuffer(f);
            }
            for t in self.ssgi_tex.iter_mut().filter_map(Option::take) {
                gl.delete_texture(t);
            }
            for i in 0..2 {
                let (Ok(t), Ok(f)) = (gl.create_texture(), gl.create_framebuffer()) else {
                    return false;
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                gl.tex_image_2d(
                    glow::TEXTURE_2D, 0, glow::RGBA16F as i32, gw, gh, 0,
                    glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None),
                );
                for (p, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
                }
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0,
                );
                gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                self.ssgi_tex[i] = Some(t);
                self.ssgi_fbo[i] = Some(f);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            self.ssgi_size = (gw, gh);
        }
        let (Some(prog), Some(blur), Some(vao), Some(vbo)) =
            (self.ssgi_prog, self.blur_rgb_prog, self.blit_vao, self.blit_vbo)
        else {
            return false;
        };
        let (Some(f0), Some(f1), Some(t0), Some(depth), Some(lit), Some(amb), Some(alb)) = (
            self.ssgi_fbo[0], self.ssgi_fbo[1], self.ssgi_tex[0],
            self.depth, self.color, self.ambient, self.albedo,
        ) else {
            return false;
        };
        const FULL: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0,  1.0, -1.0, 1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
            -1.0, -1.0, 0.0, 0.0,  1.0, 1.0, 1.0, 1.0,  -1.0, 1.0, 0.0, 1.0,
        ];
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&FULL), glow::DYNAMIC_DRAW);
        gl.bind_vertex_array(Some(vao));
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);

        // 1 — the gather.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f0));
        gl.viewport(0, 0, gw, gh);
        gl.use_program(Some(prog));
        for (unit, tex) in [
            (0, Some(depth)), (1, Some(lit)), (2, Some(amb)), (3, Some(alb)),
            (4, if ao_ready { self.ao_tex[1] } else { None }),
        ] {
            gl.active_texture(glow::TEXTURE0 + unit as u32);
            gl.bind_texture(glow::TEXTURE_2D, tex);
        }
        for (n, v) in [
            ("u_depth", 0), ("u_lit", 1), ("u_amb", 2), ("u_alb", 3), ("u_ao", 4),
            ("u_ao_on", ao_ready as i32),
            // The accumulation sample index, so the gather's jitter varies frame to frame and the
            // refinement converges to the integral instead of re-averaging one fixed set of rays.
            ("u_frame", self.taa_n as i32),
        ] {
            if let Some(l) = gl.get_uniform_location(prog, n) {
                gl.uniform_1_i32(Some(&l), v);
            }
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_vp") {
            gl.uniform_matrix_4_f32_slice(Some(&l), false, mvp);
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_inv_vp") {
            gl.uniform_matrix_4_f32_slice(Some(&l), false, inv_vp);
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_cam") {
            gl.uniform_3_f32(Some(&l), cam[0], cam[1], cam[2]);
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_radius") {
            gl.uniform_1_f32(Some(&l), gi.radius.max(0.05));
        }
        if let Some(l) = gl.get_uniform_location(prog, "u_strength") {
            gl.uniform_1_f32(Some(&l), gi.strength.clamp(0.0, 4.0));
        }
        // The FULL-resolution texel, because every fetch this shader makes is against the
        // full-resolution G-buffer even though it is drawing at half.
        if let Some(l) = gl.get_uniform_location(prog, "u_texel") {
            gl.uniform_2_f32(Some(&l), 1.0 / w as f32, 1.0 / h as f32);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.draw_arrays(glow::TRIANGLES, 0, 6);

        // 2 — blur the noise back into the broad term bounced light actually is.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f1));
        gl.use_program(Some(blur));
        gl.bind_texture(glow::TEXTURE_2D, Some(t0));
        if let Some(l) = gl.get_uniform_location(blur, "u_src") {
            gl.uniform_1_i32(Some(&l), 0);
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);

        for unit in 0..5u32 {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        true
    }

    /// The cascade shadow maps: ONE `GL_TEXTURE_2D_ARRAY` with a layer per cascade.
    ///
    /// An array rather than N separate textures because the fragment shader has to be able to pick
    /// a cascade at runtime, and GL 3.3 cannot index an array of samplers by a non-constant — a
    /// texture array is the only way to say "sample cascade `c`" in one lookup.
    unsafe fn ensure_shadow_fbo(&mut self, gl: &glow::Context) {
        if self.shadow_fbo.is_some() {
            return;
        }
        let s = self.shadow_size;
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
        gl.tex_image_3d(
            glow::TEXTURE_2D_ARRAY, 0, glow::DEPTH_COMPONENT24 as i32, s, s, CASCADE_MAX as i32, 0,
            glow::DEPTH_COMPONENT, glow::FLOAT, glow::PixelUnpackData::Slice(None),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_BORDER),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_BORDER),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, p, v as i32);
        }
        // A white border means "nothing in front of this" — a fragment that lands just off the
        // edge of a cascade reads as lit rather than as shadowed by a texel that was never drawn.
        gl.tex_parameter_f32_slice(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_BORDER_COLOR, &[1.0, 1.0, 1.0, 1.0]);

        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, Some(tex), 0, 0);
        gl.draw_buffer(glow::NONE);
        gl.read_buffer(glow::NONE);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        self.shadow_fbo = Some(fbo);
        self.shadow_tex = Some(tex);
    }

    /// Ensure a furniture instance's GPU buffer exists (shared by the main + shadow passes) and
    /// return its VAO + vertex count.
    unsafe fn furn_buf(&mut self, gl: &glow::Context, key: u64, verts: &[V3]) -> Option<(glow::VertexArray, i32)> {
        if let Some((vao, _vbo, len)) = self.furn_bufs.get(&key).copied() {
            return Some((vao, len));
        }
        let vbo = gl.create_buffer().ok()?;
        let vao = gl.create_vertex_array().ok()?;
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
        v3_attribs(gl);
        gl.bind_vertex_array(None);
        let len = verts.len() as i32;
        self.furn_bufs.insert(key, (vao, vbo, len));
        Some((vao, len))
    }

    /// Render `verts` with `mvp` into the FBO, then composite into the panel rect.
    /// `vp_*` describe the panel rect in default-framebuffer pixels (bottom-left
    /// origin); `screen_*` is the full framebuffer size in pixels.
    #[allow(clippy::too_many_arguments)]
    /// `verts` = shaded TRIANGLES · `overlay` = TRANSLUCENT triangles (selection
    /// shade + modifier ghosts) · `lines` = GL_LINES pairs (grid, boxes, axes).
    /// Pass `&[]` for `overlay`/`lines` to draw solids only — that is what the SIMLUX
    /// lighting view does, so its behaviour is unchanged.
    ///
    /// The overlay pass is separate because it needs **blending** (the main pass runs
    /// with BLEND off) and **`depth_func(LEQUAL)`**: the selection shade is exactly
    /// COINCIDENT with the solid it highlights, so under the default `LESS` it would
    /// z-fight instead of tinting. Ghost geometry sits elsewhere and blends normally.
    /// Depth WRITES are disabled so overlapping ghost faces don't occlude each other.
    ///
    /// All three passes share one program/VAO/VBO — `V3` (pos+colour) is the same
    /// vertex format throughout, so each pass just re-uploads and switches mode.
    /// Draw one opaque triangle batch. `ver = Some(v)` uses the persistent static VBO for
    /// `slot` and re-uploads only when `v` differs from the last upload; `None` re-uploads into
    /// the shared dynamic VBO every call. Caller has already set the opaque GL state.
    unsafe fn draw_opaque_batch(&mut self, gl: &glow::Context, verts: &[V3], mvp: &[f32; 16], ver: Option<u64>, slot: usize) {
        let Some(prog) = self.scene_prog else { return };
        let (vao, vbo, count) = match ver {
            Some(v) => {
                let (vao, vbo) = (self.static_vao[slot], self.static_vbo[slot]);
                let (Some(vao), Some(vbo)) = (vao, vbo) else { return };
                if self.static_ver[slot] != v {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
                    self.static_ver[slot] = v;
                    self.static_len[slot] = verts.len() as i32;
                }
                (vao, vbo, self.static_len[slot])
            }
            None => {
                let (Some(vao), Some(vbo)) = (self.scene_vao, self.scene_vbo) else { return };
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::DYNAMIC_DRAW);
                (vao, vbo, verts.len() as i32)
            }
        };
        let _ = vbo;
        gl.use_program(Some(prog));
        if let Some(loc) = &self.u_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp); }
        if let Some(loc) = &self.u_alpha { gl.uniform_1_f32(Some(loc), 1.0); }
        // The opaque scene batch is already in WORLD space → identity model for the shadow lookup.
        if let Some(loc) = &self.u_scene_model { gl.uniform_matrix_4_f32_slice(Some(loc), false, &IDENTITY16); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, count);
        gl.bind_vertex_array(None);
    }

    /// Draw one furniture instance from its own persistent GPU buffer (keyed by `key` =
    /// asset+colour). The buffer is created + uploaded the FIRST time a key is seen and reused
    /// forever after; `mvp` = camera·model, so moving/rotating is just a matrix — no CPU
    /// transform, no re-upload, regardless of triangle count. Opaque GL state already set.
    unsafe fn draw_furn(&mut self, gl: &glow::Context, key: u64, verts: &[V3], mvp: &[f32; 16], model: &[f32; 16]) {
        let Some(prog) = self.scene_prog else { return };
        let Some((vao, count)) = self.furn_buf(gl, key, verts) else { return };
        gl.use_program(Some(prog));
        if let Some(loc) = &self.u_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp); }
        if let Some(loc) = &self.u_alpha { gl.uniform_1_f32(Some(loc), 1.0); }
        // Furniture verts are LOCAL → give the shadow lookup the instance model matrix.
        if let Some(loc) = &self.u_scene_model { gl.uniform_matrix_4_f32_slice(Some(loc), false, model); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, count);
        gl.bind_vertex_array(None);
    }

    /// Draw one furniture instance's TRANSLUCENT triangles (glass panes etc.) from a persistent
    /// per-key buffer of [`V3A`]. Blend state (SRC_ALPHA / ONE_MINUS_SRC_ALPHA, depth-write off)
    /// is set once by the caller around the whole transparent pass; ordering is handled CPU-side.
    unsafe fn draw_transp(&mut self, gl: &glow::Context, key: u64, verts: &[V3A], mvp: &[f32; 16]) {
        let Some(prog) = self.transp_prog else { return };
        let entry = self.transp_bufs.get(&key).copied();
        let (vao, count) = match entry {
            Some((vao, _vbo, len)) => (vao, len),
            None => {
                let vbo = match gl.create_buffer() { Ok(b) => b, Err(_) => return };
                let vao = match gl.create_vertex_array() { Ok(v) => v, Err(_) => return };
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
                let stride = size_of::<V3A>() as i32; // 28
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
                gl.enable_vertex_attrib_array(2);
                gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 24);
                gl.bind_vertex_array(None);
                let len = verts.len() as i32;
                self.transp_bufs.insert(key, (vao, vbo, len));
                (vao, len)
            }
        };
        gl.use_program(Some(prog));
        if let Some(loc) = &self.u_transp_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, count);
        gl.bind_vertex_array(None);
    }

    /// Ensure the app-side texture `idx` (RGBA8 bytes, `w`×`h`, top row first) is uploaded to a GL
    /// texture, uploading it once and caching. Returns the GL handle.
    ///
    /// `srgb` decides the internal format, and it is not cosmetic. An **albedo** image is authored
    /// in sRGB, so it uploads as `SRGB8_ALPHA8` and the sampler decodes it to linear for free —
    /// without that, every texture in the app was being multiplied by light in the wrong space. A
    /// **data map** (tangent-space normals, roughness) carries numbers rather than colour and must
    /// stay `RGBA8`; decoding one of those would bend every normal toward the surface. The cache is
    /// keyed on the flag too, so the same image can serve as both without fighting over one upload.
    unsafe fn ensure_texture(&mut self, gl: &glow::Context, idx: usize, w: i32, h: i32, rgba: &[u8], srgb: bool) -> Option<glow::Texture> {
        if let Some(t) = self.tex_images.get(&(idx, srgb)) {
            return Some(*t);
        }
        if w <= 0 || h <= 0 || rgba.len() < (w as usize * h as usize * 4) {
            return None;
        }
        let tex = gl.create_texture().ok()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, if srgb { glow::SRGB8_ALPHA8 } else { glow::RGBA8 } as i32, w, h, 0,
            glow::RGBA, glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&rgba[..(w as usize * h as usize * 4)])),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        // Anisotropic filtering where the driver has it: a tiled floor seen at a grazing angle is
        // the worst case for trilinear alone, and it is exactly the shot people judge a render by.
        // GL 3.3 has no core anisotropy, so the extension is checked by NAME — querying the enum
        // blind would raise GL_INVALID_ENUM on drivers without it and poison later error checks.
        if self.aniso_max > 1.0 {
            gl.tex_parameter_f32(glow::TEXTURE_2D, 0x84FE, self.aniso_max); // TEXTURE_MAX_ANISOTROPY_EXT
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.tex_images.insert((idx, srgb), tex);
        Some(tex)
    }

    /// Set the procedural-material uniforms for the textured program (already bound). `mode 0`
    /// leaves the shader sampling the bound image.
    unsafe fn set_proc(&self, gl: &glow::Context, proc: &ProcParams) {
        if let Some(loc) = &self.u_tex_proc { gl.uniform_1_i32(Some(loc), proc.mode); }
        if let Some(loc) = &self.u_tex_col_a { gl.uniform_3_f32(Some(loc), proc.col_a[0], proc.col_a[1], proc.col_a[2]); }
        if let Some(loc) = &self.u_tex_col_b { gl.uniform_3_f32(Some(loc), proc.col_b[0], proc.col_b[1], proc.col_b[2]); }
        if let Some(loc) = &self.u_tex_pscale { gl.uniform_3_f32(Some(loc), proc.scale[0], proc.scale[1], proc.scale[2]); }
        if let Some(loc) = &self.u_tex_detail { gl.uniform_1_f32(Some(loc), proc.detail); }
        if let Some(loc) = &self.u_tex_prough { gl.uniform_1_f32(Some(loc), proc.rough); }
        if let Some(loc) = &self.u_tex_pcontrast { gl.uniform_1_f32(Some(loc), proc.contrast); }
        if let Some(loc) = &self.u_tex_ramp { gl.uniform_2_f32(Some(loc), proc.ramp[0], proc.ramp[1]); }
        if let Some(loc) = &self.u_tex_rough_lo { gl.uniform_1_f32(Some(loc), proc.rough_lo); }
        if let Some(loc) = &self.u_tex_rough_hi { gl.uniform_1_f32(Some(loc), proc.rough_hi); }
        if let Some(loc) = &self.u_tex_bump { gl.uniform_1_f32(Some(loc), proc.bump); }
    }

    /// Bind the PBR normal/roughness maps (units 2/3) for the textured program and set their
    /// presence flags + scalar roughness. Maps come from the app's texture cache (`tex_images`).
    unsafe fn set_pbr(&self, gl: &glow::Context, pbr: &PbrParams) {
        // Data maps, so the LINEAR upload of each (see `ensure_texture`) — never the sRGB one.
        let nrm = pbr.normal_idx.and_then(|i| self.tex_images.get(&(i, false)).copied());
        let rgh = pbr.rough_idx.and_then(|i| self.tex_images.get(&(i, false)).copied());
        let met = pbr.metal_idx.and_then(|i| self.tex_images.get(&(i, false)).copied());
        let aom = pbr.ao_idx.and_then(|i| self.tex_images.get(&(i, false)).copied());
        if let Some(loc) = &self.u_tex_has_nrm { gl.uniform_1_i32(Some(loc), nrm.is_some() as i32); }
        if let Some(loc) = &self.u_tex_has_rough { gl.uniform_1_i32(Some(loc), rgh.is_some() as i32); }
        if let Some(loc) = &self.u_tex_has_metal { gl.uniform_1_i32(Some(loc), met.is_some() as i32); }
        if let Some(loc) = &self.u_tex_has_ao { gl.uniform_1_i32(Some(loc), aom.is_some() as i32); }
        if let Some(loc) = &self.u_tex_triplanar { gl.uniform_1_i32(Some(loc), pbr.triplanar as i32); }
        if let Some(loc) = &self.u_tex_tpm { gl.uniform_1_f32(Some(loc), pbr.tiles_per_m.max(1e-3)); }
        if let Some(loc) = &self.u_tex_rough_base { gl.uniform_1_f32(Some(loc), pbr.roughness); }
        if let Some(loc) = &self.u_tex_metallic { gl.uniform_1_f32(Some(loc), pbr.metallic.clamp(0.0, 1.0)); }
        if let Some(loc) = &self.u_tex_ior { gl.uniform_1_f32(Some(loc), pbr.ior.clamp(1.0, 3.0)); }
        if let Some(loc) = &self.u_tex_coat { gl.uniform_1_f32(Some(loc), pbr.clearcoat.clamp(0.0, 1.0)); }
        if let Some(loc) = &self.u_tex_coat_rough { gl.uniform_1_f32(Some(loc), pbr.clearcoat_rough.clamp(0.01, 1.0)); }
        if let Some(loc) = &self.u_tex_sheen { gl.uniform_1_f32(Some(loc), pbr.sheen.clamp(0.0, 1.0)); }
        if let Some(loc) = &self.u_tex_sheen_tint {
            let t = pbr.sheen_tint;
            gl.uniform_3_f32(Some(loc), t[0], t[1], t[2]);
        }
        if let Some(loc) = &self.u_tex_emission { gl.uniform_3_f32(Some(loc), pbr.emission[0], pbr.emission[1], pbr.emission[2]); }
        if let Some(tex) = nrm {
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            if let Some(loc) = &self.u_tex_nrm { gl.uniform_1_i32(Some(loc), 2); }
        }
        if let Some(tex) = rgh {
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            if let Some(loc) = &self.u_tex_rough { gl.uniform_1_i32(Some(loc), 3); }
        }
        if let Some(tex) = met {
            gl.active_texture(glow::TEXTURE4);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            if let Some(loc) = &self.u_tex_metal_map { gl.uniform_1_i32(Some(loc), 4); }
        }
        if let Some(tex) = aom {
            gl.active_texture(glow::TEXTURE5);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            if let Some(loc) = &self.u_tex_ao_map { gl.uniform_1_i32(Some(loc), 5); }
        }
        gl.active_texture(glow::TEXTURE0);
    }

    /// Draw one textured mesh: `mesh_key` picks (and lazily builds) a persistent GPU buffer of
    /// `TexVtx`, `img` is the bound GL image, `mvp` = camera·model, `model` the world matrix (for
    /// shadows/normal maps). Opaque GL state already set.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_textured(&mut self, gl: &glow::Context, mesh_key: u64, verts: &[TexVtx], mvp: &[f32; 16], model: &[f32; 16], img: glow::Texture, cam: [f32; 3], reflect: f32, proc: ProcParams, pbr: PbrParams, hl: bool) {
        let Some(prog) = self.tex_prog else { return };
        let entry = self.tex_bufs.get(&mesh_key).copied();
        let (vao, count) = match entry {
            Some((vao, _vbo, len)) => (vao, len),
            None => {
                let vbo = match gl.create_buffer() { Ok(b) => b, Err(_) => return };
                let vao = match gl.create_vertex_array() { Ok(v) => v, Err(_) => return };
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
                let stride = size_of::<TexVtx>() as i32; // 28
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 12);
                gl.enable_vertex_attrib_array(2);
                gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 20);
                gl.enable_vertex_attrib_array(3);
                gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 24);
                gl.bind_vertex_array(None);
                let len = verts.len() as i32;
                self.tex_bufs.insert(mesh_key, (vao, vbo, len));
                (vao, len)
            }
        };
        gl.use_program(Some(prog));
        if let Some(loc) = &self.u_tex_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp); }
        if let Some(loc) = &self.u_tex_model { gl.uniform_matrix_4_f32_slice(Some(loc), false, model); }
        if let Some(loc) = &self.u_tex_cam { gl.uniform_3_f32(Some(loc), cam[0], cam[1], cam[2]); }
        if let Some(loc) = &self.u_tex_reflect { gl.uniform_1_f32(Some(loc), reflect); }
        if let Some(loc) = &self.u_tex_hl { gl.uniform_1_i32(Some(loc), hl as i32); }
        self.set_proc(gl, &proc);
        self.set_pbr(gl, &pbr);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(img));
        if let Some(loc) = &self.u_tex_img { gl.uniform_1_i32(Some(loc), 0); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, count);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }

    /// Draw a textured mesh from the SHARED DYNAMIC buffer (re-uploaded every call) — for
    /// world-space feature surfaces whose geometry changes on recompute. `mvp` = scene matrix and
    /// the geometry is already world-space, so the world model is the identity.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_textured_dyn(&mut self, gl: &glow::Context, verts: &[TexVtx], mvp: &[f32; 16], img: glow::Texture, cam: [f32; 3], reflect: f32, proc: ProcParams, pbr: PbrParams, hl: bool) {
        let (Some(prog), Some(vao), Some(vbo)) = (self.tex_prog, self.tex_dyn_vao, self.tex_dyn_vbo) else { return };
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::DYNAMIC_DRAW);
        gl.use_program(Some(prog));
        if let Some(loc) = &self.u_tex_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp); }
        if let Some(loc) = &self.u_tex_model { gl.uniform_matrix_4_f32_slice(Some(loc), false, &IDENTITY16); }
        if let Some(loc) = &self.u_tex_cam { gl.uniform_3_f32(Some(loc), cam[0], cam[1], cam[2]); }
        if let Some(loc) = &self.u_tex_reflect { gl.uniform_1_f32(Some(loc), reflect); }
        if let Some(loc) = &self.u_tex_hl { gl.uniform_1_i32(Some(loc), hl as i32); }
        self.set_proc(gl, &proc);
        self.set_pbr(gl, &pbr);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(img));
        if let Some(loc) = &self.u_tex_img { gl.uniform_1_i32(Some(loc), 0); }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, verts.len() as i32);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }

    pub fn render(
        &mut self,
        gl: &glow::Context,
        verts: &[V3],
        overlay: &[V3],
        lines: &[V3],
        mvp: &[f32; 16],
        // `scene_ver`: when `Some(v)`, `verts` is drawn from a persistent buffer that is
        // re-uploaded ONLY when `v` changes (kills the per-frame upload of a static heavy
        // scene). `None` = re-upload every frame (the small SIMLUX light view).
        scene_ver: Option<u64>,
        // Furniture instances, each `(key, local_mesh, camera·model)`. Every furniture is drawn
        // from a persistent per-key GPU buffer with a model matrix — so a heavy mesh is uploaded
        // once and never CPU-transformed on import/move/rotate. `key` = asset+colour; instances
        // that share it share the buffer. `local_mesh` is only read the first time a key appears.
        // `(key, local_mesh, camera·model, world model)` — the world model added for the shadow
        // lookup + normal maps (identity's fine for already-world geometry).
        furn: &[(u64, &[V3], [f32; 16], [f32; 16])],
        // TRANSLUCENT furniture triangles, each `(key, local_mesh, camera·model)` like `furn` but
        // holding only the see-through faces (glass). Uploaded once per key, drawn in a blended
        // pass after all opaque geometry; the caller passes them BACK-TO-FRONT for correct order.
        transp: &[(u64, &[V3A], [f32; 16])],
        // Textures referenced by `tex_draws` this frame, as `(index, w, h, rgba)` — uploaded
        // to GL once and cached by index, so re-pasting the same texture never re-uploads.
        tex_assets: &[(usize, i32, i32, &[u8])],
        // Textured draws: `(texture_index, mesh_key, &[TexVtx], camera·model, world model)`.
        tex_draws: &[(usize, u64, &[TexVtx], [f32; 16], [f32; 16])],
        // TRANSLUCENT textured draws (textured glass): same tuple as `tex_draws` but drawn in the
        // blended pass after all opaque geometry, back-to-front, so the image shows through.
        tex_transp: &[(usize, u64, &[TexVtx], [f32; 16], [f32; 16])],
        // Textured FEATURE surfaces (world-space, `(texture_index, &[TexVtx])`), re-uploaded
        // each frame and drawn with the scene `mvp` (they carry no per-object model matrix).
        tex_feat: &[(usize, &[TexVtx])],
        // TRANSLUCENT textured feature surfaces (a CSG solid whose texture opacity < 1): same as
        // `tex_feat` but drawn in the blended pass, back-to-front, so the solid reads see-through.
        tex_feat_transp: &[(usize, &[TexVtx])],
        // Camera world position (for the reflection sheen) and per-texture REFLECTION amounts
        // `(texture_index, reflect 0..1)`. A texture absent here / reflect 0 = matte.
        cam_pos: [f32; 3],
        tex_reflect: &[(usize, f32)],
        // Per-texture PROCEDURAL material params `(texture_index, ProcParams)`. A texture absent
        // here (or mode 0) samples its bound image as before; mode > 0 evaluates a world-space
        // noise→ramp pattern in the shader instead (wood grain, marble, …).
        tex_proc: &[(usize, ProcParams)],
        // Per-texture PBR maps `(texture_index, PbrParams)` — tangent-space normal + roughness
        // maps for the textured pass (Texture Phase 2). Absent / all-None renders as before.
        tex_pbr: &[(usize, PbrParams)],
        // DAYLIGHT: `Some((dir, sun_rgb))` lights every surface by the sun (dir points TO the sun)
        // instead of the baked studio scalar. The AMBIENT half no longer travels with it — that is
        // now the sky in `env`, integrated properly rather than lerped between two colours.
        // `None` = the fixed studio light (unchanged).
        sun: Option<([f32; 3], [f32; 3])>,
        // Sun SHADOWS, as CASCADES: one light matrix per cascade, TIGHTEST FIRST. Empty (or `sun`
        // None) = no cast shadows. One entry reproduces the single whole-scene map exactly.
        //
        // Ordering is load-bearing: the fragment shader walks them in order and uses the first that
        // contains the fragment, so the tightest map has to come first or every fragment would be
        // shadowed by the coarsest one available.
        shadow_mvp: &[[f32; 16]],
        // CLAY mode: force textured/procedural surfaces to flat grey (glass keeps its alpha). The
        // flat V3 scene/furniture are greyed CPU-side (see `factory::set_clay`); this covers the
        // textured pass, which samples its albedo in-shader.
        clay: bool,
        // MATERIAL HIGHLIGHT: `Some((texture_index, pulse 0..1))` tints every surface using that
        // material cyan (pulsing) so the Materials Factory selection is visible in the scene.
        highlight: Option<(usize, f32)>,
        // COLOUR MANAGEMENT. Every pass writes scene-referred LINEAR light into an RGBA16F target;
        // this decides how that becomes pixels at the composite. Pass
        // [`crate::color::ColorPipeline::passthrough`] for a view whose vertex colours are already
        // display values (the SIMLUX lux heatmap) — re-grading a false-colour scale corrupts it.
        color: crate::color::ColorPipeline,
        // ENVIRONMENT. The analytic sky that lights the scene (its 9 SH coefficients drive every
        // diffuse ambient, its radiance every glossy reflection), what to draw behind the model,
        // and the ambient-occlusion settings. [`crate::env::EnvRender::none`] reproduces the
        // studio-only behaviour exactly, which is what the SIMLUX lighting view passes.
        env: crate::env::EnvRender,
        // When true, delete any furniture/texture GPU buffers + GL images NOT referenced this
        // frame (recolour/delete/re-texture leave stale ones). The factory passes true; the
        // SIMLUX light view passes false so its empty lists don't wipe the factory's buffers
        // (both share one renderer).
        evict_stale: bool,
        vp_left: i32,
        vp_from_bottom: i32,
        vp_w: i32,
        vp_h: i32,
        screen_w: i32,
        screen_h: i32,
    ) {
        if vp_w <= 0 || vp_h <= 0 {
            return;
        }
        self.ensure_init(gl);

        // ---- TEMPORAL ACCUMULATION: has anything changed? ----------------
        // Built from this function's OWN parameters, so no input can be left out of the question
        // (see [`FrameKey`]). Anything different from last frame throws the history away and the
        // refinement starts over from one clean sample.
        //
        // Bulk geometry that the renderer CACHES by key or version is keyed the same way here —
        // hashing content the renderer would not re-upload anyway would only invent differences.
        // Geometry it re-uploads every frame (overlays, lines, feature surfaces) is hashed whole.
        let taa_on = self.taa_max > 0;
        if taa_on {
            let mut f = Fnv::new();
            let mut dbg = std::mem::take(&mut self.taa_dbg);
            f.f32s(mvp);
            f.f32s(&cam_pos);
            f.u64(self.env_version);
            match scene_ver {
                Some(v) => f.u64(v),
                None => f.bytes(bytes(verts)),
            }
            f.bytes(bytes(overlay));
            f.bytes(bytes(lines));
            for (k, m, mv, md) in furn {
                f.u64(*k);
                f.u64(m.len() as u64);
                f.f32s(mv);
                f.f32s(md);
            }
            for (k, m, mv) in transp {
                f.u64(*k);
                f.u64(m.len() as u64);
                f.f32s(mv);
            }
            for (i, w, h, px) in tex_assets {
                f.u64(*i as u64);
                f.u64(((*w as u64) << 32) | *h as u64);
                f.u64(px.len() as u64);
            }
            for (ti, k, m, mv, md) in tex_draws.iter().chain(tex_transp) {
                f.u64(*ti as u64);
                f.u64(*k);
                f.u64(m.len() as u64);
                f.f32s(mv);
                f.f32s(md);
            }
            for (ti, m) in tex_feat.iter().chain(tex_feat_transp) {
                f.u64(*ti as u64);
                f.bytes(bytes(m));
            }
            f.dbg(&mut dbg, &(sun, shadow_mvp, clay, highlight, color, env));
            for e in tex_reflect {
                f.dbg(&mut dbg, e);
            }
            for e in tex_proc {
                f.dbg(&mut dbg, e);
            }
            for e in tex_pbr {
                f.dbg(&mut dbg, e);
            }
            self.taa_dbg = dbg;
            let key = FrameKey { hash: f.0, size: (vp_w, vp_h) };
            self.taa_stable = key == self.taa_key;
            if !self.taa_stable {
                self.taa_key = key;
                self.taa_n = 0;
            }
        } else {
            self.taa_n = 0;
            self.taa_stable = false;
        }

        // Sub-pixel camera jitter, from sample 1 onwards. Sample 0 is deliberately UNJITTERED, so
        // the first frame after any change is bit-for-bit the image this renderer drew before TAA
        // existed — a drag or an orbit never sees the picture shift under the cursor.
        let jittered;
        let mvp: &[f32; 16] = if taa_on && self.taa_n >= 1 {
            let (jx, jy) = halton_jitter(self.taa_n);
            jittered = jitter_mvp(mvp, jx, jy, vp_w, vp_h);
            &jittered
        } else {
            mvp
        };

        // SOFT SUN SHADOWS. Same idea, one dimension up: each accumulation sample sees the sun
        // from a different point on its disc, so the average is the disc's own penumbra. Applied
        // AFTER the frame key, deliberately — the jitter is how the frame is being refined, not a
        // change to what the frame is, and folding it into the key would restart the refinement
        // every frame and guarantee it never converged.
        let spun_cascades;
        let (sun, shadow_mvp): (_, &[[f32; 16]]) = match sun {
            Some((d, col)) if taa_on && self.taa_n >= 1 && env.sun_angle_deg > 0.0 => {
                let half = env.sun_angle_deg.to_radians() * 0.5;
                match sun_disc_sample(d, half, self.taa_n) {
                    (d2, Some(spin)) => {
                        // The SAME rotation for every cascade. Drawing an independent sample per
                        // cascade would light each slice of the view from a slightly different sun,
                        // and the seam where two cascades meet would flicker.
                        spun_cascades =
                            shadow_mvp.iter().map(|m| rotate_light(*m, spin)).collect::<Vec<_>>();
                        (Some((d2, col)), &spun_cascades[..])
                    }
                    _ => (sun, shadow_mvp),
                }
            }
            _ => (sun, shadow_mvp),
        };

        unsafe {
            self.ensure_fbo(gl, vp_w, vp_h);

            // Converged, and nothing changed: the finished image is already sitting in the
            // accumulation buffer, so present it and skip the scene entirely. This is what makes
            // an idle heavy viewport nearly free — the alternative is redrawing a million
            // triangles to arrive at a pixel-identical result.
            if taa_on && self.taa_valid && self.taa_n >= self.taa_max {
                if let Some(src) = self.taa_tex[self.taa_cur] {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    gl.viewport(0, 0, screen_w.max(1), screen_h.max(1));
                    gl.enable(glow::SCISSOR_TEST);
                    gl.disable(glow::DEPTH_TEST);
                    gl.disable(glow::BLEND);
                    let quad = panel_quad(vp_left, vp_from_bottom, vp_w, vp_h, screen_w, screen_h);
                    // `composed` = true, so the fog arguments are inert here — the accumulated
                    // buffer already has it.
                    self.composite(
                        gl, &quad, src, true, false, false, false, color, &env.fog, cam_pos, mvp,
                        (vp_left, vp_from_bottom, vp_w, vp_h),
                    );
                    release_scissor(gl, screen_w, screen_h);
                    gl.enable(glow::BLEND);
                    gl.use_program(None);
                    self.note_geom(
                        gl, (vp_left, vp_from_bottom, vp_w, vp_h), (screen_w, screen_h), &env, 0,
                    );
                    return;
                }
            }

            // ---- evict GPU resources no longer referenced ----------------
            // Deleting/recolouring/re-texturing an object orphans its per-key buffer; without
            // this they accumulate for the whole session. Keyed on what THIS frame draws.
            if evict_stale {
                use std::collections::HashSet;
                let live_furn: HashSet<u64> = furn.iter().map(|&(k, _, _, _)| k).collect();
                self.furn_bufs.retain(|k, &mut (vao, vbo, _)| {
                    if live_furn.contains(k) { return true; }
                    gl.delete_vertex_array(vao);
                    gl.delete_buffer(vbo);
                    false
                });
                let live_transp: HashSet<u64> = transp.iter().map(|&(k, _, _)| k).collect();
                self.transp_bufs.retain(|k, &mut (vao, vbo, _)| {
                    if live_transp.contains(k) { return true; }
                    gl.delete_vertex_array(vao);
                    gl.delete_buffer(vbo);
                    false
                });
                let live_tex: HashSet<u64> = tex_draws.iter().map(|&(_, k, _, _, _)| k)
                    .chain(tex_transp.iter().map(|&(_, k, _, _, _)| k)).collect();
                self.tex_bufs.retain(|k, &mut (vao, vbo, _)| {
                    if live_tex.contains(k) { return true; }
                    gl.delete_vertex_array(vao);
                    gl.delete_buffer(vbo);
                    false
                });
                let live_img: HashSet<usize> = tex_assets.iter().map(|&(i, _, _, _)| i).collect();
                self.tex_images.retain(|&(k, _srgb), &mut tex| {
                    if live_img.contains(&k) { return true; }
                    gl.delete_texture(tex);
                    false
                });
            }

            // ---- SUN SHADOW MAP: depth pass from the sun's point of view ----
            // Only when daylight + shadows are requested. Renders the opaque casters (scene +
            // furniture) into the depth-only shadow FBO with the light-space matrix; the main
            // passes below sample it. Front-face culling during the depth pass curbs shadow acne.
            let cascades = &shadow_mvp[..shadow_mvp.len().min(CASCADE_MAX)];
            let do_shadow = sun.is_some() && !cascades.is_empty();
            if do_shadow {
                self.ensure_shadow_fbo(gl);
                if let (Some(dprog), Some(sfbo), Some(stex)) = (self.depth_prog, self.shadow_fbo, self.shadow_tex) {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sfbo));
                    // SCISSOR OFF, and this is the whole ballgame.
                    //
                    // egui hands the callback a scissor set to the 3D panel's rect in WINDOW
                    // coordinates, and it is still set here. The shadow map is a 2048² texture with
                    // its own coordinate space, so that rectangle lands somewhere arbitrary inside
                    // it — and `glClear` obeys the scissor just as draws do. The result is a depth
                    // map that is only cleared and only drawn inside a window-shaped patch; every
                    // texel outside it keeps whatever was in that memory, which reads as "an
                    // occluder is standing right here" and comes out as hard black shadow.
                    //
                    // Because the patch is fixed in WINDOW space while the map is in LIGHT space,
                    // moving the camera slides the scene across the boundary — so the black region
                    // swims about as you orbit and the sun looks welded to the viewpoint. It is not:
                    // the sun's direction never sees the camera. This is the shadow map being
                    // partly undrawn.
                    gl.disable(glow::SCISSOR_TEST);
                    gl.viewport(0, 0, self.shadow_size, self.shadow_size);
                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_func(glow::LESS);
                    gl.disable(glow::BLEND);
                    gl.enable(glow::CULL_FACE);
                    gl.cull_face(glow::FRONT);
                    gl.use_program(Some(dprog));
                    // The scene's static buffer is uploaded ONCE, outside the cascade loop — three
                    // cascades must not mean three re-uploads of a million-triangle villa.
                    let scene_ready = !verts.is_empty()
                        && match (scene_ver, self.static_vao[0], self.static_vbo[0]) {
                            (Some(v), Some(_), Some(vbo)) => {
                                if self.static_ver[0] != v {
                                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
                                    self.static_ver[0] = v;
                                    self.static_len[0] = verts.len() as i32;
                                }
                                true
                            }
                            _ => false,
                        };
                    for (ci, lmvp) in cascades.iter().enumerate() {
                        // Re-point the FBO at this cascade's layer of the array and clear IT — the
                        // clear has to be inside the loop or every cascade after the first would be
                        // drawn on top of the one before.
                        gl.framebuffer_texture_layer(
                            glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, Some(stex), 0, ci as i32,
                        );
                        gl.clear(glow::DEPTH_BUFFER_BIT);
                        // Scene (world-space) → depth_mvp = light_mvp.
                        if scene_ready {
                            if let Some(vao) = self.static_vao[0] {
                                if let Some(loc) = &self.u_depth_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, lmvp); }
                                gl.bind_vertex_array(Some(vao));
                                gl.draw_arrays(glow::TRIANGLES, 0, self.static_len[0]);
                            }
                        }
                        // Furniture (local) → depth_mvp = light_mvp · model.
                        let lm = Mat4::from_cols_array(lmvp);
                        for &(key, fverts, _fmvp, ref model) in furn {
                            if let Some((vao, count)) = self.furn_buf(gl, key, fverts) {
                                let dm = (lm * Mat4::from_cols_array(model)).to_cols_array();
                                if let Some(loc) = &self.u_depth_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &dm); }
                                gl.bind_vertex_array(Some(vao));
                                gl.draw_arrays(glow::TRIANGLES, 0, count);
                            }
                        }
                    }
                    gl.cull_face(glow::BACK);
                    gl.disable(glow::CULL_FACE);
                    gl.bind_vertex_array(None);
                }
            }

            // ---- per-frame sun + shadow uniforms on the scene & textured programs ----
            // Bind the shadow map on unit 1 for both programs to sample.
            if do_shadow {
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, self.shadow_tex);
                gl.active_texture(glow::TEXTURE0);
            }
            // The HDR environment stays bound for the whole frame — the backdrop, the solid pass
            // and the textured pass all read it from the same unit, so binding it once here is
            // simpler than three binds and cannot get out of step with the uniform.
            if self.env_tex.is_some() {
                gl.active_texture(glow::TEXTURE0 + ENV_UNIT as u32);
                gl.bind_texture(glow::TEXTURE_2D, self.env_tex);
                gl.active_texture(glow::TEXTURE0 + ENV_UNIT as u32 + 1);
                // Falls back to the chain if the full-resolution copy could not be created (a 4K
                // RGB16F is 50 MB). Softer than intended beats an unbound sampler, which reads as
                // black and looks like the environment failed to load.
                gl.bind_texture(glow::TEXTURE_2D, self.env_bg_tex.or(self.env_tex));
                gl.active_texture(glow::TEXTURE0);
            }
            // The cascade matrices as ONE flat upload — a `mat4[3]` uniform takes 48 contiguous
            // floats. Unused cascades are left as identity; `u_csm_n` stops the shader reading them.
            let mut lmvps = [0.0f32; CASCADE_MAX * 16];
            for c in 0..CASCADE_MAX {
                let m = cascades.get(c).copied().unwrap_or(IDENTITY16);
                lmvps[c * 16..(c + 1) * 16].copy_from_slice(&m);
            }
            let csm_n = cascades.len() as i32;
            // The SOLID passes carry scene-referred linear light already (the CPU shader decodes the
            // material's sRGB before multiplying it by irradiance), so they must not be decoded
            // again. Only the overlay and line passes carry authored sRGB — UI swatches — and they
            // set this back to 1 just before they draw.
            let lin = color.linearize_vertex as i32;
            if let Some(prog) = self.scene_prog {
                gl.use_program(Some(prog));
                self.sky_u_scene.set(gl, &env, ENV_UNIT);
                if let Some(loc) = &self.u_scene_shadow_on { gl.uniform_1_i32(Some(loc), do_shadow as i32); }
                if let Some(loc) = &self.u_scene_light_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &lmvps); }
                if let Some(loc) = &self.u_scene_csm_n { gl.uniform_1_i32(Some(loc), csm_n); }
                if let Some(loc) = &self.u_scene_shadow { gl.uniform_1_i32(Some(loc), 1); }
                if let Some(loc) = &self.u_scene_linearize { gl.uniform_1_i32(Some(loc), 0); }
            }
            if let Some(prog) = self.transp_prog {
                gl.use_program(Some(prog));
                if let Some(loc) = &self.u_transp_linearize { gl.uniform_1_i32(Some(loc), 0); }
            }
            if let Some(prog) = self.tex_prog {
                gl.use_program(Some(prog));
                let on = sun.is_some();
                if let Some(loc) = &self.u_tex_sun_on { gl.uniform_1_i32(Some(loc), on as i32); }
                if let Some((d, sc)) = sun {
                    if let Some(loc) = &self.u_tex_sun_dir { gl.uniform_3_f32(Some(loc), d[0], d[1], d[2]); }
                    if let Some(loc) = &self.u_tex_sun_col { gl.uniform_3_f32(Some(loc), sc[0], sc[1], sc[2]); }
                }
                self.sky_u_tex.set(gl, &env, ENV_UNIT);
                if let Some(loc) = &self.u_tex_shadow_on { gl.uniform_1_i32(Some(loc), do_shadow as i32); }
                if let Some(loc) = &self.u_tex_light_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &lmvps); }
                if let Some(loc) = &self.u_tex_csm_n { gl.uniform_1_i32(Some(loc), csm_n); }
                if let Some(loc) = &self.u_tex_shadow { gl.uniform_1_i32(Some(loc), 1); }
                if let Some(loc) = &self.u_tex_clay { gl.uniform_1_i32(Some(loc), clay as i32); }
                // Highlight pulse strength once per frame; per-draw u_hl flags the matching material.
                if let Some(loc) = &self.u_tex_hl_k {
                    gl.uniform_1_f32(Some(loc), highlight.map(|(_, k)| k).unwrap_or(0.0));
                }
                if let Some(loc) = &self.u_tex_hl { gl.uniform_1_i32(Some(loc), 0); }
            }

            // ---- 3D pass into the offscreen FBO --------------------------
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.fbo);
            gl.disable(glow::SCISSOR_TEST); // FBO is 1:1 with the rect; clear all of it
            gl.viewport(0, 0, vp_w, vp_h);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.disable(glow::BLEND);
            // The FBO holds linear light now, so the backdrop is authored in sRGB and decoded — a
            // Raw pipeline keeps the literal bytes it always had.
            let bg = if color.linearize_vertex {
                crate::color::srgb_to_linear3([0.07, 0.086, 0.11])
            } else {
                [0.07, 0.086, 0.11]
            };
            // Per-attachment clears: the backdrop belongs in the DIRECT buffer, and the ambient
            // buffer starts empty. A single `clear_color` would have written the studio grey into
            // both, and the composite would then have added it to itself.
            gl.clear_buffer_f32_slice(glow::COLOR, 0, &[bg[0], bg[1], bg[2], 1.0]);
            gl.clear_buffer_f32_slice(glow::COLOR, 1, &[0.0, 0.0, 0.0, 1.0]);
            // Albedo clears to ZERO, which means "background, nothing bounces here". A clear to
            // grey would make every pixel the model does not cover into a bounce card.
            gl.clear_buffer_f32_slice(glow::COLOR, 2, &[0.0, 0.0, 0.0, 1.0]);
            gl.clear(glow::DEPTH_BUFFER_BIT);

            // ---- sky backdrop -------------------------------------------
            // Drawn over the cleared background, before any geometry, with depth off. Anything the
            // model covers is simply overwritten, so it costs one full-screen pass and no sorting.
            // An HDR ENVIRONMENT counts as a sky in its own right. This used to require a valid
            // analytic dome, which meant an HDRI drew nothing whenever daylight was switched off or
            // the sun was below the horizon — and in an empty scene the backdrop is the entire
            // image, so the environment looked as though it had failed to load at all.
            let have_sky = env.hdri.is_some() || env.sky.map(|s| s.valid).unwrap_or(false);
            if env.backdrop == crate::env::Backdrop::Sky && have_sky {
                if let (Some(prog), Some(vao), Some(vbo)) = (self.sky_prog, self.blit_vao, self.blit_vbo) {
                    const FULL: [f32; 24] = [
                        -1.0, -1.0, 0.0, 0.0,  1.0, -1.0, 1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
                        -1.0, -1.0, 0.0, 0.0,  1.0,  1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0,
                    ];
                    let inv = Mat4::from_cols_array(mvp).inverse().to_cols_array();
                    gl.disable(glow::DEPTH_TEST);
                    gl.depth_mask(false);
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&FULL), glow::DYNAMIC_DRAW);
                    gl.use_program(Some(prog));
                    if let Some(l) = &self.u_sky_inv_vp { gl.uniform_matrix_4_f32_slice(Some(l), false, &inv); }
                    if let Some(l) = &self.u_sky_cam { gl.uniform_3_f32(Some(l), cam_pos[0], cam_pos[1], cam_pos[2]); }
                    self.sky_u_bg.set(gl, &env, ENV_UNIT);
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, 6);
                    gl.bind_vertex_array(None);
                    gl.depth_mask(true);
                    gl.enable(glow::DEPTH_TEST);
                }
            }

            if !verts.is_empty() {
                self.draw_opaque_batch(gl, verts, mvp, scene_ver, 0);
            }

            // ---- furniture pass: every instance, its own model matrix ----
            // Same opaque state as the scene. Each furniture draws from a persistent GPU buffer
            // (uploaded once, keyed by asset+colour) with camera·model — so import/move/rotate
            // of even a multi-million-triangle piece needs no CPU transform and no re-upload.
            for &(key, verts, ref fmvp, ref model) in furn {
                self.draw_furn(gl, key, verts, fmvp, model);
            }

            // ---- textured pass: objects carrying a pasted image ----------
            // Same opaque state. Upload any referenced textures once, then draw each textured
            // mesh from its persistent buffer with the bound image + camera·model matrix.
            // Per-texture reflection / procedural / PBR-map lookups (defaults when absent).
            let reflect_of = |idx: usize| tex_reflect.iter().find(|(i, _)| *i == idx).map(|(_, r)| *r).unwrap_or(0.0);
            let proc_of = |idx: usize| tex_proc.iter().find(|(i, _)| *i == idx).map(|(_, p)| *p).unwrap_or_default();
            let pbr_of = |idx: usize| tex_pbr.iter().find(|(i, _)| *i == idx).map(|(_, p)| *p).unwrap_or_default();
            let hl_of = |idx: usize| highlight.map(|(h, _)| h == idx).unwrap_or(false);
            if !tex_draws.is_empty() || !tex_feat.is_empty() || !tex_transp.is_empty() || !tex_feat_transp.is_empty() {
                // Which indices are used as DATA maps this frame — they need a second, linear
                // upload. Most textures are albedo-only and get just the sRGB one.
                let data_maps: std::collections::HashSet<usize> =
                    tex_pbr.iter().flat_map(|(_, p)| [p.normal_idx, p.rough_idx, p.metal_idx, p.ao_idx]).flatten().collect();
                for &(idx, w, h, rgba) in tex_assets {
                    let _ = self.ensure_texture(gl, idx, w, h, rgba, true);
                    if data_maps.contains(&idx) {
                        let _ = self.ensure_texture(gl, idx, w, h, rgba, false);
                    }
                }
                // Furniture (persistent per-key buffers + model matrix).
                for &(tex_idx, mesh_key, verts, ref tmvp, ref model) in tex_draws {
                    if let Some(img) = self.tex_images.get(&(tex_idx, true)).copied() {
                        self.draw_textured(gl, mesh_key, verts, tmvp, model, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
                // Feature surfaces (dynamic, world-space, scene mvp).
                for &(tex_idx, verts) in tex_feat {
                    if let Some(img) = self.tex_images.get(&(tex_idx, true)).copied() {
                        self.draw_textured_dyn(gl, verts, mvp, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
            }

            // ---- transparent furniture pass — glass panes etc. ----------
            // Depth-TESTED against the opaque geometry already in the buffer (so glass behind a
            // wall is correctly hidden) but with depth writes OFF, so overlapping translucent
            // faces all show through instead of the nearest one occluding the rest. The caller
            // hands `transp` back-to-front, which is the ordering blending needs.
            if !transp.is_empty() || !tex_transp.is_empty() || !tex_feat_transp.is_empty() {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.depth_mask(false);
                for &(key, verts, ref tmvp) in transp {
                    self.draw_transp(gl, key, verts, tmvp);
                }
                // Textured glass (image shows through). Drawn after the flat glass; the images
                // were uploaded in the textured pass above.
                for &(tex_idx, mesh_key, verts, ref tmvp, ref model) in tex_transp {
                    if let Some(img) = self.tex_images.get(&(tex_idx, true)).copied() {
                        self.draw_textured(gl, mesh_key, verts, tmvp, model, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
                // See-through CSG feature solids (world-space, scene mvp); caller sorts back-to-front.
                for &(tex_idx, verts) in tex_feat_transp {
                    if let Some(img) = self.tex_images.get(&(tex_idx, true)).copied() {
                        self.draw_textured_dyn(gl, verts, mvp, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
                gl.depth_mask(true);
                gl.disable(glow::BLEND);
            }

            // ---- overlay pass — selection shade + modifier ghosts --------
            // LEQUAL (not LESS): the shade is coincident with the solid it tints, so
            // LESS would z-fight. Depth-write off so ghost faces don't occlude each
            // other. Alpha comes from the vertex colour's implied blend below.
            if !overlay.is_empty() {
                if let (Some(prog), Some(vao), Some(vbo)) =
                    (self.scene_prog, self.scene_vao, self.scene_vbo)
                {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                    gl.depth_func(glow::LEQUAL);
                    gl.depth_mask(false);
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(overlay), glow::DYNAMIC_DRAW);
                    gl.use_program(Some(prog));
                    if let Some(loc) = &self.u_scene_shadow_on { gl.uniform_1_i32(Some(loc), 0); } // overlays never shadowed
                    if let Some(loc) = &self.u_scene_linearize { gl.uniform_1_i32(Some(loc), lin); } // UI colours are sRGB
                    if let Some(loc) = &self.u_mvp {
                        gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp);
                    }
                    if let Some(loc) = &self.u_alpha {
                        gl.uniform_1_f32(Some(loc), 0.45); // translucent shade / ghost
                    }
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, overlay.len() as i32);
                    gl.bind_vertex_array(None);
                    gl.depth_mask(true);
                    gl.depth_func(glow::LESS);
                    gl.disable(glow::BLEND);
                }
            }

            // ---- line pass (grid / selection / ghosts) -------------------
            // Same program + VAO/VBO as above: identical `V3` vertex layout, so we
            // only re-upload and switch the primitive mode. Depth-tested with the
            // solids so lines are correctly occluded by them.
            if !lines.is_empty() {
                if let (Some(prog), Some(vao), Some(vbo)) =
                    (self.scene_prog, self.scene_vao, self.scene_vbo)
                {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(lines), glow::DYNAMIC_DRAW);
                    gl.use_program(Some(prog));
                    if let Some(loc) = &self.u_scene_shadow_on { gl.uniform_1_i32(Some(loc), 0); } // lines never shadowed
                    if let Some(loc) = &self.u_scene_linearize { gl.uniform_1_i32(Some(loc), lin); } // UI colours are sRGB
                    if let Some(loc) = &self.u_mvp {
                        gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp);
                    }
                    if let Some(loc) = &self.u_alpha {
                        gl.uniform_1_f32(Some(loc), 1.0); // opaque
                    }
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_arrays(glow::LINES, 0, lines.len() as i32);
                    gl.bind_vertex_array(None);
                }
            }

            // ---- ambient occlusion over the finished depth buffer --------
            // After all geometry (so every draw kind is represented) and before the composite (so
            // the result is available when the two light buffers are added back together).
            let ao_ready = env.ao.enabled && self.run_ssao(gl, &env.ao, mvp, cam_pos, vp_w, vp_h);

            // ---- post passes, then composite into the panel rect ---------
            // The scissor stays OFF through the post chain. egui's scissor rect is the panel in
            // SCREEN coordinates, while bloom and the accumulator render into their own targets
            // starting at (0, 0) — leaving it enabled clips whatever falls outside the panel's
            // screen position, so a viewport anywhere but the bottom-left corner gets a pyramid
            // that is partly never written. It goes back on for the composite, which really does
            // draw into the panel rect and must be clipped to it.
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);

            let quad = panel_quad(vp_left, vp_from_bottom, vp_w, vp_h, screen_w, screen_h);
            // BLOOM, over the finished scene buffer and before the composite reads it. It writes
            // into its own pyramid, restores the default framebuffer, and reports whether the
            // composite may sample the result.
            let bloom_ready = self.bloom_pass(gl, vp_w as i32, vp_h as i32, ao_ready, color);

            // ACCUMULATE. The resolve composes this sample exactly as the composite would, averages
            // it into the history, and hands back the running mean for the composite to grade.
            // The depth buffer was written with the JITTERED camera, so the fog reconstructs world
            // positions with the same matrix — otherwise every accumulation sample would place the
            // scene a fraction of a pixel elsewhere and the fog would shimmer along silhouettes.
            let inv_vp = Mat4::from_cols_array(mvp).inverse().to_cols_array();
            // ONE BOUNCE of coloured light between visible surfaces, gathered from the G-buffer.
            // Before the accumulation resolve, so its noise is averaged away by the same sixteen
            // samples that anti-alias the image — a gather this sparse needs that.
            let ssgi_ready =
                self.run_ssgi(gl, &env.gi, ao_ready, mvp, &inv_vp, cam_pos, vp_w, vp_h);
            let accumulated = if taa_on {
                let t = self.taa_resolve(
                    gl, vp_w, vp_h, ao_ready, bloom_ready, ssgi_ready, color, &env.fog, cam_pos,
                    &inv_vp, &env.gi,
                );
                if t.is_none() {
                    // The resolve could not run — no buffers, or the program failed to compile.
                    // Turn accumulation OFF outright rather than marking it converged: "converged"
                    // would send the next frame down the re-present path, which would show a
                    // buffer nothing has ever written and leave the viewport black for good.
                    // This way the scene is simply drawn every frame, exactly as it did before
                    // accumulation existed.
                    self.taa_max = 0;
                    self.taa_n = 0;
                    self.taa_valid = false;
                }
                t
            } else {
                None
            };

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, screen_w.max(1), screen_h.max(1)); // restore egui's full-screen viewport
            gl.enable(glow::SCISSOR_TEST); // egui's scissor (= this rect) is still set

            // Present the accumulated mean when there is one, otherwise this frame's own buffer.
            if let Some(src) = accumulated.or(self.color) {
                let composed = accumulated.is_some();
                self.composite(
                    gl, &quad, src, composed, ao_ready, bloom_ready, ssgi_ready, color, &env.fog,
                    cam_pos, &inv_vp, (vp_left, vp_from_bottom, vp_w, vp_h),
                );
            }

            // Leave egui's expected state: blend on, program unbound, scissor wide open.
            release_scissor(gl, screen_w, screen_h);
            gl.enable(glow::BLEND);
            gl.use_program(None);
            self.note_geom(
                gl, (vp_left, vp_from_bottom, vp_w, vp_h), (screen_w, screen_h), &env,
                cascades.len(),
            );
        }
    }

}

/// Compile + link one program, returning `None` (and reporting to stderr) if the driver rejects it.
///
/// This used to `panic!`. A panic here happens inside the egui paint callback, on the GL thread, at
/// the first 3D frame — the window is already up, so the user sees the app die rather than a shader
/// error, and every remaining feature dies with it. There is no way to compile GLSL in a unit test
/// (no context), so the failure mode is real and the honest response is to lose ONE program: the
/// feature it drove stops drawing, the rest of the viewport still works, and the log says which.
unsafe fn compile(gl: &glow::Context, name: &str, vs_src: &str, fs_src: &str) -> Option<glow::Program> {
    let program = gl.create_program().ok()?;
    let compile_one = |src: &str, kind: u32, stage: &str| -> Option<glow::Shader> {
        let s = gl.create_shader(kind).ok()?;
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            eprintln!("SIMLUX 3D: {name} {stage} shader failed to compile:\n{}", gl.get_shader_info_log(s));
            gl.delete_shader(s);
            return None;
        }
        Some(s)
    };
    let Some(vs) = compile_one(vs_src, glow::VERTEX_SHADER, "vertex") else {
        gl.delete_program(program);
        return None;
    };
    let Some(fs) = compile_one(fs_src, glow::FRAGMENT_SHADER, "fragment") else {
        gl.delete_shader(vs);
        gl.delete_program(program);
        return None;
    };
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    let linked = gl.get_program_link_status(program);
    if !linked {
        eprintln!("SIMLUX 3D: {name} program failed to link:\n{}", gl.get_program_info_log(program));
    }
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    if linked {
        Some(program)
    } else {
        gl.delete_program(program);
        None
    }
}

/// Bind the [`V3`] attribute layout (position, colour, ambient share) on the currently bound VAO +
/// array buffer. One function because three separate call sites used to spell out the same offsets,
/// and adding a field to `V3` meant remembering all three.
unsafe fn v3_attribs(gl: &glow::Context) {
    let stride = size_of::<V3>() as i32; // 40
    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
    gl.enable_vertex_attrib_array(2);
    gl.vertex_attrib_pointer_f32(2, 3, glow::FLOAT, false, stride, 24); // normal
    gl.enable_vertex_attrib_array(3);
    gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 36); // shading mode
}

/// What the last [`Scene3dRenderer::render`] actually drew with — the numbers that decide WHERE
/// pixels land, and whether the buffers they land in are the size everyone thinks they are.
///
/// Every one of these is invisible from outside the paint callback, which is exactly why a
/// mismatch between them is so hard to reason about from a screenshot: a viewport that disagrees
/// with its framebuffer, or an accumulation buffer left at a stale size, both come out as "part of
/// the picture is wrong" with nothing to point at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameGeom {
    /// `(left, from_bottom, width, height)` in physical pixels — the rect egui gave the callback,
    /// and the rect the composite scissors and draws to.
    pub vp: (i32, i32, i32, i32),
    pub screen: (i32, i32),
    /// The offscreen colour/depth target. Must equal the viewport's size.
    pub fbo: (i32, i32),
    /// The accumulation buffers. Must equal the viewport's size too — a stale size here means the
    /// resolve writes a corner of them and the composite stretches the rest over the panel.
    pub taa: (i32, i32),
    pub taa_valid: bool,
    pub taa_n: u32,
    pub taa_max: u32,
    /// Top of the bloom pyramid — half the viewport, or (0, 0) when bloom is off.
    pub bloom0: (i32, i32),
    /// `GL_FRAMEBUFFER_COMPLETE` for the offscreen target. False means every 3D draw this frame
    /// was silently discarded by the driver.
    pub fbo_complete: bool,
    /// An HDR environment was in use — the one thing that makes the symptom visible, because
    /// without it every stray pixel is dark and reads as background.
    pub env: bool,
    /// Cascades rendered this frame.
    pub cascades: u32,
}

impl std::fmt::Display for FrameGeom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "vp={},{} {}×{} screen={}×{} fbo={}×{}{} taa={}×{} n={}/{}{} bloom0={}×{} csm={} env={}",
            self.vp.0, self.vp.1, self.vp.2, self.vp.3,
            self.screen.0, self.screen.1,
            self.fbo.0, self.fbo.1,
            if self.fbo_complete { "" } else { " INCOMPLETE!" },
            self.taa.0, self.taa.1, self.taa_n, self.taa_max,
            if self.taa_valid { "" } else { " (empty)" },
            self.bloom0.0, self.bloom0.1, self.cascades, self.env,
        )
    }
}

/// The largest value a 16-bit float can hold. Above it, an upload becomes `+Infinity`.
pub const HALF_MAX: f32 = 65504.0;

/// Bring one radiance sample into half-float range on its way to the GPU.
///
/// An HDRI's sun really is brighter than this. Poly Haven's `kloofendal_48d_partly_cloudy` peaks at
/// **75360**; a clear-sky panorama reaches six figures. Uploaded to an RGB16F texture unchanged,
/// every one of those texels becomes `+Infinity` — and infinity does not stay put. The bloom bright
/// pass divides by the pixel's own brightness, `Inf / Inf` is NaN, and the pyramid's blur then
/// smears that NaN across a wide, power-of-two-aligned block, which the driver finally writes as
/// solid black. One texel of sun becomes a black rectangle that moves as the sun crosses the frame.
///
/// The clamp costs nothing visible: 65504 is orders of magnitude past what the view transform rolls
/// off to white. And it costs nothing in LIGHTING either — the spherical harmonics and the path
/// tracer both read the original full-precision map on the CPU, never this copy.
fn half_safe(v: &f32) -> f32 {
    // NaNs occur in real EXR files, and would propagate exactly as badly.
    if v.is_nan() { 0.0 } else { v.clamp(-HALF_MAX, HALF_MAX) }
}

/// Open the scissor back up to the whole window before handing control back to egui.
///
/// THE NEXT FRAME'S CLEAR IS THE REASON. eframe clears the window at the start of a frame, before
/// egui has set any clip rect — and `glClear` obeys the scissor box, which is still whatever the
/// last thing to touch it left behind. Leave ours set to the 3D panel and the next clear wipes only
/// that rectangle: the panel goes black, and every pixel outside it keeps the PREVIOUS frame's
/// image. That is exactly "a black rectangle where the 3D view is, with a stale sky frozen around
/// it, and the boundary moving as I zoom" — the shape of the union of recent panel rects.
///
/// egui re-enables scissoring and sets a rect per primitive, so it neither needs nor notices this;
/// the only consumer is that clear, and it needs the whole window.
fn release_scissor(gl: &glow::Context, screen_w: i32, screen_h: i32) {
    unsafe {
        gl.scissor(0, 0, screen_w.max(1), screen_h.max(1));
    }
}

/// The six vertices (pos.xy, uv) that map the viewport texture onto the panel rect it belongs to.
fn panel_quad(vp_left: i32, vp_from_bottom: i32, vp_w: i32, vp_h: i32, screen_w: i32, screen_h: i32) -> [f32; 24] {
    let sw = screen_w.max(1) as f32;
    let sh = screen_h.max(1) as f32;
    let x0 = 2.0 * vp_left as f32 / sw - 1.0;
    let x1 = 2.0 * (vp_left + vp_w) as f32 / sw - 1.0;
    let y0 = 2.0 * vp_from_bottom as f32 / sh - 1.0;
    let y1 = 2.0 * (vp_from_bottom + vp_h) as f32 / sh - 1.0;
    [
        x0, y0, 0.0, 0.0,  x1, y0, 1.0, 0.0,  x1, y1, 1.0, 1.0,
        x0, y0, 0.0, 0.0,  x1, y1, 1.0, 1.0,  x0, y1, 0.0, 1.0,
    ]
}

/// Reinterpret a `&[T]` of `Copy` POD as bytes, for `glBufferData`.
fn bytes<T: Copy>(slice: &[T]) -> &[u8] {
    let len = std::mem::size_of_val(slice);
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, len) }
}

#[cfg(test)]
mod taa_tests {
    use super::*;

    /// Project a world point through a matrix and return where it lands, in PIXELS.
    fn project(m: &[f32; 16], p: Vec3, w: i32, h: i32) -> (f32, f32) {
        let c = Mat4::from_cols_array(m) * p.extend(1.0);
        (
            (c.x / c.w * 0.5 + 0.5) * w as f32,
            (c.y / c.w * 0.5 + 0.5) * h as f32,
        )
    }

    fn camera() -> [f32; 16] {
        let proj = Mat4::perspective_rh_gl(0.9, 16.0 / 9.0, 0.1, 500.0);
        let view = Mat4::look_at_rh(Vec3::new(6.0, -8.0, 4.0), Vec3::ZERO, Vec3::Z);
        (proj * view).to_cols_array()
    }

    /// The jitter must move the image by EXACTLY the number of pixels asked for. If it does not,
    /// the samples cluster instead of covering the pixel and the accumulation blurs rather than
    /// anti-aliases.
    #[test]
    fn the_jitter_moves_the_image_by_the_pixels_it_was_given() {
        let (w, h) = (1600, 900);
        let m = camera();
        let p = Vec3::new(1.0, 2.0, 0.5);
        let base = project(&m, p, w, h);
        for (jx, jy) in [(0.5, -0.25), (-0.5, 0.5), (0.125, 0.375)] {
            let j = jitter_mvp(&m, jx, jy, w, h);
            let moved = project(&j, p, w, h);
            assert!((moved.0 - base.0 - jx).abs() < 1e-2, "x moved {} px, wanted {jx}", moved.0 - base.0);
            assert!((moved.1 - base.1 - jy).abs() < 1e-2, "y moved {} px, wanted {jy}", moved.1 - base.1);
        }
    }

    /// …and by the same amount at EVERY depth.
    ///
    /// This is the whole reason the offset is folded into clip space against the `w` row rather
    /// than added after the perspective divide. Get it wrong and near geometry jitters further
    /// than far geometry, so accumulating sixteen samples smears the background while sharpening
    /// the foreground — which reads as a focus artefact nobody would trace back to anti-aliasing.
    #[test]
    fn the_jitter_is_the_same_size_at_every_depth() {
        let (w, h) = (1600, 900);
        let m = camera();
        let j = jitter_mvp(&m, 0.4, -0.3, w, h);
        let near = Vec3::new(0.2, 0.1, 0.0);
        let far = Vec3::new(20.0, 40.0, 3.0);
        let dn = {
            let (a, b) = (project(&m, near, w, h), project(&j, near, w, h));
            (b.0 - a.0, b.1 - a.1)
        };
        let df = {
            let (a, b) = (project(&m, far, w, h), project(&j, far, w, h));
            (b.0 - a.0, b.1 - a.1)
        };
        assert!((dn.0 - df.0).abs() < 1e-2 && (dn.1 - df.1).abs() < 1e-2,
            "near moved {dn:?}, far moved {df:?} — the shift is depth-dependent");
    }

    /// Zero jitter must be the identity, because sample 0 is deliberately unjittered: the first
    /// frame after any change has to be bit-for-bit the image this renderer drew before TAA
    /// existed, or every drag would start with the picture shifting under the cursor.
    #[test]
    fn no_jitter_changes_nothing() {
        let m = camera();
        assert_eq!(jitter_mvp(&m, 0.0, 0.0, 1600, 900), m);
    }

    /// Every sample lands inside its own pixel, and they spread out rather than clustering.
    #[test]
    fn the_samples_cover_the_pixel() {
        let mut quadrants = [0u32; 4];
        for i in 1..=16u32 {
            let (x, y) = halton_jitter(i);
            assert!(x.abs() <= 0.5 && y.abs() <= 0.5, "sample {i} at ({x}, {y}) left the pixel");
            quadrants[((x > 0.0) as usize) | (((y > 0.0) as usize) << 1)] += 1;
        }
        assert!(quadrants.iter().all(|&n| n >= 3), "samples bunched into a corner: {quadrants:?}");
        // Halton's point is that SHORT prefixes are already even, so the image looks right long
        // before the last sample arrives.
        let (mut sx, mut sy) = (0.0, 0.0);
        for i in 1..=4u32 {
            let (x, y) = halton_jitter(i);
            sx += x;
            sy += y;
        }
        assert!(sx.abs() < 0.5 && sy.abs() < 0.5, "the first four samples are already lopsided");
    }

    /// The running blend must be a TRUE mean, not an exponential fade.
    ///
    /// With a fixed blend factor the result never converges and keeps a trace of the first frame
    /// forever — so a viewport that had been orbiting would settle to something slightly wrong and
    /// stay there. 1/(n+1) makes the buffer the exact unweighted average of every sample so far.
    #[test]
    fn the_accumulation_is_a_true_mean() {
        let samples: Vec<f32> = (0..16).map(|i| i as f32 * 0.1 + 0.5).collect();
        let mut acc = 0.0f32;
        for (n, &s) in samples.iter().enumerate() {
            let b = 1.0 / (n as f32 + 1.0);
            acc = acc * (1.0 - b) + s * b; // exactly what `mix(hist, cur, u_blend)` computes
        }
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!((acc - mean).abs() < 1e-5, "accumulated {acc}, true mean {mean}");
        assert!((1.0f32 / 1.0 - 1.0).abs() < 1e-9, "the first sample must replace, not blend");
    }

    /// The resolve has to compose the frame EXACTLY as the composite would, or turning
    /// accumulation on would change what the picture is — not just how clean it is.
    #[test]
    fn the_accumulator_composes_the_image_the_composite_does() {
        for term in ["u_amb", "u_ao", "u_ao_on", "u_bloom_k"] {
            assert!(TAA_FS.contains(term), "the resolve reads {term}, as the composite does");
        }
        assert!(TAA_FS.contains("c.rgb + a * ao"), "composed identically");
        assert!(TAA_FS.contains("if (u_bloom_k > 0.0) lit += texture(u_bloom, v_uv).rgb * u_bloom_k;"),
            "bloom folded in identically");
    }

    /// Averaging must happen in scene-referred LINEAR light, before the view transform.
    ///
    /// AgX is a curve, and the mean of a curve is not the curve of the mean. It matters most for
    /// what this unlocks: a noisy signal — soft shadows, screen-space GI — averaged after the tone
    /// map converges to the wrong answer, not merely a slightly different one.
    #[test]
    fn accumulation_happens_before_the_view_transform() {
        assert!(!TAA_FS.contains("apply_view"), "the resolve must not tone-map");
        assert!(!TAA_FS.contains("VIEW_GLSL"), "…nor carry the view transform at all");
        assert!(BLIT_FS.contains("apply_view(lit)"), "the composite is still the one place it happens");
    }

    /// …and the composite must not add ambient, occlusion or bloom a second time to a buffer that
    /// already has them folded in.
    #[test]
    fn the_composite_does_not_double_count_accumulated_light() {
        assert!(BLIT_FS.contains("u_composed"), "the composite is told when its input is composed");
        assert!(BLIT_FS.contains("(u_composed == 1) ? c.rgb : c.rgb + a * ao"),
            "…and skips the ambient it already contains");
    }

    /// A changed input must produce a different key. This is the whole safety property: if a key
    /// collides across a real change, the viewport shows a stale image and looks fine doing it.
    #[test]
    fn a_changed_input_changes_the_key() {
        let base = {
            let mut f = Fnv::new();
            f.f32s(&[1.0, 2.0, 3.0]);
            f.0
        };
        let moved = {
            let mut f = Fnv::new();
            f.f32s(&[1.0, 2.000_001, 3.0]);
            f.0
        };
        assert_ne!(base, moved, "a sub-millimetre camera move went unnoticed");
        // Length is part of the hash, so a shorter run of the same values is a different frame.
        let mut a = Fnv::new();
        a.bytes(&[1, 2, 3]);
        let mut b = Fnv::new();
        b.bytes(&[1, 2, 3, 0]);
        assert_ne!(a.0, b.0, "a trailing zero vanished from the key");
    }

    /// Settings structs go into the key through `Debug`, which prints every field — so a field
    /// added later joins the key automatically instead of quietly falling out of it.
    #[test]
    fn every_field_of_a_settings_struct_is_in_the_key() {
        let mut buf = String::new();
        let hash = |p: &dyn std::fmt::Debug, buf: &mut String| {
            let mut f = Fnv::new();
            f.dbg(buf, p);
            f.0
        };
        let base = ProcParams::default();
        let h0 = hash(&base, &mut buf);
        // Every field, one at a time — including the ones added last, which is exactly the class
        // of field a hand-written hash forgets.
        let mut variants = Vec::new();
        let mut v = base; v.mode = 2; variants.push(v);
        let mut v = base; v.col_a = [0.1, 0.2, 0.3]; variants.push(v);
        let mut v = base; v.col_b = [0.1, 0.2, 0.3]; variants.push(v);
        let mut v = base; v.scale = [2.0, 1.0, 1.0]; variants.push(v);
        let mut v = base; v.detail += 1.0; variants.push(v);
        let mut v = base; v.rough += 0.25; variants.push(v);
        let mut v = base; v.contrast += 0.25; variants.push(v);
        let mut v = base; v.ramp = [0.1, 0.9]; variants.push(v);
        let mut v = base; v.rough_lo += 0.25; variants.push(v);
        let mut v = base; v.rough_hi += 0.25; variants.push(v);
        let mut v = base; v.bump += 0.5; variants.push(v);
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(h0, hash(v, &mut buf), "ProcParams variant {i} hashed the same as the default");
        }
        // The colour pipeline too — including bloom, which changes the image without touching a
        // single vertex.
        let c0 = crate::color::ColorPipeline::default();
        let mut c1 = c0;
        c1.bloom += 0.1;
        assert_ne!(hash(&c0, &mut buf), hash(&c1, &mut buf), "a bloom change went unnoticed");
        let mut c2 = c0;
        c2.exposure += 0.5;
        assert_ne!(hash(&c0, &mut buf), hash(&c2, &mut buf), "an exposure change went unnoticed");
    }

    /// The key covers the viewport SIZE separately, because the accumulation buffers are
    /// viewport-sized: a resize invalidates the history even if every other input is identical.
    #[test]
    fn a_resize_invalidates_the_history() {
        let a = FrameKey { hash: 7, size: (800, 600) };
        let b = FrameKey { hash: 7, size: (800, 601) };
        assert_ne!(a, b);
    }

    /// An orthographic light matrix looking straight down at the origin, covering ±20 m.
    fn sun_overhead() -> [f32; 16] {
        let proj = Mat4::orthographic_rh_gl(-20.0, 20.0, -20.0, 20.0, 1.0, 200.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 100.0), Vec3::ZERO, Vec3::Y);
        (proj * view).to_cols_array()
    }

    /// Where a world point lands in the shadow map, in light-space texture coordinates.
    fn shadow_uv(m: &[f32; 16], p: Vec3) -> (f32, f32) {
        let c = Mat4::from_cols_array(m) * p.extend(1.0);
        (c.x / c.w, c.y / c.w)
    }

    /// The sun's disc must move the shadow of a HIGH occluder further than a low one.
    ///
    /// This is the entire reason for jittering the light rather than blurring the shadow map. A
    /// blur has one width everywhere: set it soft enough for the eaves and the contact shadow under
    /// a chair leg detaches from the leg; set it tight enough for the leg and the eaves stay razor
    /// sharp. Sampling the disc gets the widening for free and gets it right.
    #[test]
    fn a_high_occluder_gets_a_wider_penumbra_than_a_low_one() {
        let up = [0.0f32, 0.0, 1.0];
        let m = sun_overhead();
        let half = 5.0f32.to_radians(); // exaggerated so the test reads in metres, not microns
        let (mut low, mut high) = (0.0f32, 0.0f32);
        for i in 1..=16 {
            let (_, s) = jitter_sun(up, Some(m), half, i);
            let s = s.expect("the shadow matrix travels with the sun");
            // Two points on the SAME vertical line, one near the ground and one well above it.
            let a = shadow_uv(&s, Vec3::new(0.0, 0.0, 0.5));
            let b = shadow_uv(&s, Vec3::new(0.0, 0.0, 10.0));
            let base = shadow_uv(&m, Vec3::new(0.0, 0.0, 0.5));
            let base_b = shadow_uv(&m, Vec3::new(0.0, 0.0, 10.0));
            low = low.max((a.0 - base.0).hypot(a.1 - base.1));
            high = high.max((b.0 - base_b.0).hypot(b.1 - base_b.1));
        }
        assert!(low > 0.0, "the sun did not move at all");
        assert!(
            high > low * 3.0,
            "a 10 m occluder swept {high:.4} and a 0.5 m one {low:.4} — the penumbra is not widening with height"
        );
    }

    /// The samples must cover the disc, and stay ON it.
    ///
    /// Escaping the disc would put the sun somewhere it is not and lighten shadows that should be
    /// fully dark; clustering in the middle would make the penumbra too tight at its edges.
    #[test]
    fn the_sun_samples_stay_on_their_own_disc() {
        let dir = Vec3::new(0.3, -0.6, 0.74).normalize();
        let half = 2.0f32.to_radians();
        let mut spread = 0.0f32;
        for i in 1..=32 {
            let (d, _) = jitter_sun([dir.x, dir.y, dir.z], None, half, i);
            let d = Vec3::from(d);
            assert!((d.length() - 1.0).abs() < 1e-4, "sample {i} is not a unit direction");
            let a = d.dot(dir).clamp(-1.0, 1.0).acos();
            assert!(a <= half * 1.001, "sample {i} left the disc: {a} > {half}");
            spread = spread.max(a);
        }
        assert!(spread > half * 0.8, "every sample huddled near the centre of the disc");
    }

    /// Sample 0 and a zero-width sun must both leave everything exactly as it was.
    ///
    /// The first frame after any change is the one the user sees while dragging; it has to be the
    /// image this renderer has always drawn, not a version lit from a slightly wrong direction.
    #[test]
    fn no_disc_means_no_change() {
        let dir = [0.0f32, 0.0, 1.0];
        let m = sun_overhead();
        let (d, s) = jitter_sun(dir, Some(m), 0.0, 5);
        assert_eq!(d, dir, "a zero-width sun moved");
        assert_eq!(s, Some(m), "…and dragged the shadow matrix with it");
    }

    /// The disc jitter must NOT be part of the frame key.
    ///
    /// It is how the frame is being refined, not a change to what the frame IS. Folding it in
    /// would restart the accumulation every single frame, so the refinement could never converge —
    /// and the sun would appear to shimmer instead of the shadows going soft.
    #[test]
    fn the_sun_jitter_is_applied_after_the_frame_key() {
        let src = include_str!("light3d.rs");
        let key = src.find("let key = FrameKey { hash: f.0").expect("the frame key");
        let jit = src.find("let (sun, shadow_mvp) = match sun {").expect("the sun jitter");
        assert!(key < jit, "the sun is jittered before the key is taken — accumulation cannot converge");
    }

    /// The fog integral, transcribed from the GLSL — the only way to check it without a context.
    fn transmittance(cam: Vec3, p: Vec3, density: f32, base: f32, falloff: f32) -> f32 {
        if density <= 0.0 {
            return 1.0;
        }
        let seg = p - cam;
        let l = seg.length();
        if l < 1e-4 {
            return 1.0;
        }
        let rho = density * (-falloff * (cam.z - base)).exp();
        let kdz = falloff * seg.z;
        let tau = if kdz.abs() < 1e-4 { rho * l } else { rho * l * (1.0 - (-kdz).exp()) / kdz };
        (-tau.max(0.0)).exp()
    }

    /// Fog must thin with height, and looking UP through it must pick up less than looking level.
    ///
    /// This is the whole reason for height fog over plain distance fog: haze pools in a valley and
    /// the hills above it stay clear. Uniform fog gets that backwards — it puts as much air between
    /// you and a rooftop as between you and the road.
    #[test]
    fn haze_pools_low_and_thins_with_height() {
        let (d, base, k) = (0.004f32, 0.0f32, 0.05f32);
        let cam = Vec3::new(0.0, 0.0, 2.0);
        let level = transmittance(cam, cam + Vec3::new(200.0, 0.0, 0.0), d, base, k);
        let upward = transmittance(cam, cam + Vec3::new(150.0, 0.0, 132.0), d, base, k); // same length
        assert!(upward > level, "looking up through the haze picked up MORE of it ({upward} vs {level})");
        // …and a camera up on a roof sees a clearer world than one at street level.
        let high = Vec3::new(0.0, 0.0, 60.0);
        let from_roof = transmittance(high, high + Vec3::new(200.0, 0.0, 0.0), d, base, k);
        assert!(from_roof > level, "the air did not thin with altitude");
    }

    /// A level look is where the closed form divides by zero — take the limit, don't compute it.
    ///
    /// Get this wrong and a bright seam appears along the horizon at exactly eye height, which is
    /// the one place in the frame a viewer is guaranteed to be looking.
    #[test]
    fn a_level_look_has_no_seam() {
        let (d, k) = (0.003f32, 0.04f32);
        let cam = Vec3::new(0.0, 0.0, 1.6);
        let flat = transmittance(cam, cam + Vec3::new(300.0, 0.0, 0.0), d, 0.0, k);
        assert!(flat.is_finite() && flat > 0.0 && flat < 1.0, "a level look gave {flat}");
        // Approaching level from both sides converges on it — no discontinuity to fall into.
        for eps in [1e-3f32, 1e-4, 1e-5] {
            for sign in [1.0f32, -1.0] {
                let t = transmittance(cam, cam + Vec3::new(300.0, 0.0, sign * eps), d, 0.0, k);
                assert!((t - flat).abs() < 5e-3, "Δz={} jumped to {t} from {flat}", sign * eps);
            }
        }
    }

    /// Distance is what fog is FOR: further must always mean hazier.
    #[test]
    fn further_is_always_hazier() {
        let cam = Vec3::new(0.0, 0.0, 1.6);
        let mut last = 1.0;
        for m in [10.0f32, 50.0, 100.0, 400.0, 1000.0] {
            let t = transmittance(cam, cam + Vec3::new(m, 0.0, 0.0), 0.002, 0.0, 0.01);
            assert!(t < last, "{m} m was no hazier than the step before it");
            last = t;
        }
        assert!(last < 0.2, "a kilometre of haze barely touched the image ({last})");
    }

    /// Zero density must be a true no-op — the setting has to be genuinely free when off.
    #[test]
    fn no_fog_is_no_change() {
        let cam = Vec3::new(3.0, -2.0, 1.5);
        assert_eq!(transmittance(cam, cam + Vec3::new(500.0, 20.0, -30.0), 0.0, 0.0, 0.05), 1.0);
    }

    /// The composite and the accumulator must apply fog the SAME way, and neither twice.
    ///
    /// The accumulated buffer already has fog folded in; adding it again at the composite would
    /// square the transmittance and put a second helping of inscatter on top, so switching the
    /// refinement on would visibly change the weather.
    #[test]
    fn fog_is_applied_once_and_identically() {
        for src in [TAA_FS, BLIT_FS] {
            assert!(src.contains("apply_fog(lit, v_uv)"), "both compose fog through one helper");
            assert!(src.contains("FOG_GLSL"), "…and both include the shared block");
        }
        let block = fog_glsl();
        assert!(!block.contains("FOG_GLSL_BODY"), "the integral is spliced in");
        assert!(block.contains("fog_transmittance"), "…and is the one from crate::env");
        assert!(block.contains("if (u_fog_on != 1) return lit;"), "off is a real early-out");
        assert!(
            block.contains("if (d >= 0.999999) return lit;"),
            "the far plane is sky, not a surface — fogging it flattens the backdrop"
        );
    }

    /// Every cascade must be turned by the SAME sample of the sun's disc.
    ///
    /// Drawing an independent sample per cascade would light each slice of the view from a
    /// slightly different sun. The cascades overlap, so the seam between two of them would show a
    /// step in every shadow — and it would flicker, because the two samples change independently
    /// each frame. That is much worse than the softness it was meant to buy.
    #[test]
    fn one_sun_sample_turns_every_cascade() {
        let m = sun_overhead();
        let up = [0.0f32, 0.0, 1.0];
        let half = 3.0f32.to_radians();
        let (_, spin) = sun_disc_sample(up, half, 4);
        let spin = spin.expect("a wide disc yields a rotation");
        // The same rotation applied to two differently-fitted cascades must move a shared world
        // point by the same amount in each — that is what "one sun" means.
        let tight = Mat4::orthographic_rh_gl(-5.0, 5.0, -5.0, 5.0, 1.0, 200.0)
            * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 100.0), Vec3::ZERO, Vec3::Y);
        let a = rotate_light(m, spin);
        let b = rotate_light(tight.to_cols_array(), spin);
        // Recover each one's implied light direction by seeing where a tall point's shadow lands.
        let shift = |before: &[f32; 16], after: &[f32; 16], scale: f32| {
            let p = Vec3::new(0.0, 0.0, 10.0);
            let f = |m: &[f32; 16]| {
                let c = Mat4::from_cols_array(m) * p.extend(1.0);
                (c.x / c.w * scale, c.y / c.w * scale)
            };
            let (x0, y0) = f(before);
            let (x1, y1) = f(after);
            (x1 - x0, y1 - y0)
        };
        // Scaled back to WORLD units by each cascade's own half-extent, so the two are comparable.
        let sa = shift(&m, &a, 20.0);
        let sb = shift(&tight.to_cols_array(), &b, 5.0);
        assert!(
            (sa.0 - sb.0).abs() < 0.05 && (sa.1 - sb.1).abs() < 0.05,
            "cascades moved by {sa:?} and {sb:?} — they are not seeing the same sun"
        );
    }

    /// The shader must try cascades TIGHTEST-FIRST and use the first that contains the fragment.
    ///
    /// Selecting by distance instead is the classic way to get a hard black line across a floor at
    /// a cascade boundary: a fragment near the edge of the frame is further from the camera than
    /// the split says, lands outside the cascade it was assigned, and reads whatever happens to be
    /// in that texel. Containment cannot do that.
    #[test]
    fn cascades_are_selected_by_containment() {
        let g = shadow_glsl();
        assert!(g.contains(&format!("u_light_mvp[{CASCADE_MAX}]")), "the array is sized by Rust");
        assert!(!g.contains("CASCADE_MAX_GLSL"), "…and the token is fully substituted");
        assert!(g.contains("sampler2DArray"), "one array texture, so the layer can be chosen at runtime");
        // The out-of-bounds branch must CONTINUE to the next cascade, not return.
        let body = g.split("for (int c = 0").nth(1).expect("the cascade loop");
        let oob = body.find("p.x < 0.0").expect("the bounds test");
        let cont = body[oob..].find("continue;").expect("…falls through to the next cascade");
        let ret = body[oob..].find("return 1.0;").unwrap_or(usize::MAX);
        assert!(cont < ret, "a fragment outside a cascade gives up instead of trying the next");
        assert!(g.trim_end().ends_with("}"), "…and the loop ends with an unshadowed fallback");
    }

    /// The shadow depth pass must run with egui's SCISSOR OFF.
    ///
    /// egui leaves the scissor set to the 3D panel's rect in WINDOW coordinates. The shadow map is
    /// a 2048² texture with its own coordinate space, so that rectangle lands somewhere arbitrary
    /// inside it — and `glClear` obeys the scissor exactly as draws do. Leave it on and the depth
    /// map is only cleared and only drawn inside a window-shaped patch; every texel outside keeps
    /// whatever was in that memory, which reads as an occluder standing right there and comes out
    /// as hard black shadow.
    ///
    /// The giveaway is that the patch is fixed in WINDOW space while the map is in LIGHT space, so
    /// orbiting slides the scene across its boundary and the black region swims with the camera —
    /// which looks precisely like the sun being pinned to the viewpoint.
    #[test]
    fn the_shadow_pass_runs_outside_egui_scissor() {
        let src = include_str!("light3d.rs");
        let bind = src
            .find("gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sfbo));")
            .expect("the shadow FBO bind");
        let clear = src[bind..].find("gl.clear(glow::DEPTH_BUFFER_BIT)").expect("the depth clear") + bind;
        let off = src[bind..clear].find("gl.disable(glow::SCISSOR_TEST)");
        assert!(
            off.is_some(),
            "the shadow map is cleared and drawn while egui's window-space scissor is still active"
        );
    }

    /// Bounced light must be coloured by what it LANDS on, not just by what it left.
    ///
    /// This is the whole reason the albedo G-buffer exists. Every other buffer holds light already
    /// multiplied by albedo, inseparably, so a bounce computed without it would land identically
    /// on a white floor and a black one — which reads as fog, not as bounce.
    #[test]
    fn bounced_light_is_multiplied_by_the_receiver_albedo() {
        assert!(SSGI_FS.contains("u_alb"), "the gather reads the albedo buffer");
        assert!(SSGI_FS.contains("frag = vec4(alb * gi"), "…and multiplies the bounce by it");
        // A surface with no albedo is background, glass or a UI swatch: it must bounce nothing
        // rather than a black-but-present term.
        assert!(
            SSGI_FS.contains("if (alb.r + alb.g + alb.b <= 0.0) { frag = vec4(0.0); return; }"),
            "a zero-albedo pixel still contributes"
        );
    }

    /// Light must not pour through walls.
    ///
    /// A gather that only checks the RECEIVER's cosine happily reads the far side of a wall and
    /// lights the room next door through it — the single most recognisable screen-space GI
    /// failure, and the reason the emitter's normal is reconstructed at all.
    #[test]
    fn the_emitter_must_face_the_receiver() {
        assert!(SSGI_FS.contains("normal_at(su, Q)"), "the emitter's normal is reconstructed");
        assert!(
            SSGI_FS.contains("if (-dot(normal_at(su, Q), to) <= 0.0) continue;"),
            "…and a back-facing emitter is dropped"
        );
        // The reconstruction must NOT use screen derivatives: the point is fetched from a texture,
        // so its derivative across the quad describes nothing.
        let f = SSGI_FS.find("vec3 normal_at(").expect("the reconstruction");
        let body = &SSGI_FS[f..f + 320];
        assert!(!body.contains("dFdx"), "the emitter normal is taken from a texture-fetched value");
        // The sky is not a bounce card either — it is already lighting the scene through its SH.
        assert!(SSGI_FS.contains(">= 0.99999) continue;"), "the sky is gathered from");
    }

    /// The receiver's cosine must be applied ONCE.
    ///
    /// The gather draws directions cosine-weighted (`cos = sqrt(1 - a)`), so the receiver's cosine
    /// and the diffuse BRDF's 1/π are already carried by the sampling — the estimator is albedo
    /// times the plain MEAN of what those directions saw. Multiplying by `dot(N, to)` as well
    /// applies it twice, and the two together average about a third, which is enough to make the
    /// whole effect invisible at a strength of 1. It is the kind of mistake that reads as "the
    /// slider is too weak" rather than as a bug, so it is worth pinning.
    #[test]
    fn the_receiver_cosine_is_not_applied_twice() {
        // Cosine-weighted sampling, which is what carries the cosine.
        assert!(SSGI_FS.contains("ct = sqrt(1.0 - a)"), "the directions are not cosine-weighted");
        // The accumulation line must not re-apply it.
        let acc = SSGI_FS
            .lines()
            .find(|l| l.contains("gi +="))
            .expect("the accumulation");
        assert!(
            !acc.contains("cos_r") && !acc.contains("dot(N, to)"),
            "the receiver cosine is applied a second time when accumulating: {acc}"
        );
        // …and the emitter test must be a rejection too, not a weight.
        assert!(
            !acc.contains("cos_e") && !acc.contains("normal_at"),
            "the emitter cosine is used as a weight rather than a rejection: {acc}"
        );
    }

    /// The gather's jitter must vary with the accumulation sample, or the refinement converges to
    /// its own bias: sixteen frames averaging the same eight directions is still eight directions.
    #[test]
    fn the_gather_decorrelates_across_accumulation_samples() {
        assert!(SSGI_FS.contains("u_frame"), "the gather has no frame index");
        let f = SSGI_FS.find("float rot =").expect("the per-pixel spin");
        assert!(
            SSGI_FS[f..f + 200].contains("u_frame"),
            "the spin does not vary with the accumulation sample"
        );
    }

    /// Both composition shaders must fold the bounce in identically, and before the view transform.
    #[test]
    fn the_bounce_is_composed_the_same_in_both_shaders() {
        let line = "if (u_ssgi_k > 0.0) lit += texture(u_ssgi, v_uv).rgb * u_ssgi_k;";
        assert!(BLIT_FS.contains(line), "the composite folds the bounce in");
        assert!(TAA_FS.contains(line), "…and the accumulator does it identically");
        let add = BLIT_FS.find("u_ssgi_k").expect("the composite adds it");
        let view = BLIT_FS.find("apply_view(lit)").expect("…then the view transform");
        assert!(add < view, "bounced light goes in before the transform, not after");
    }

    /// It must be OFF by default — a viewpoint-dependent term has no business appearing in
    /// someone's scene without being asked for, least of all in a lighting application.
    #[test]
    fn bounced_light_is_opt_in() {
        assert!(!crate::env::GiSettings::default().enabled, "GI is on by default");
        assert!(!crate::env::EnvRender::default().gi.enabled);
        // …and the lux view, which is a measurement, must never get it at all.
        assert!(!crate::env::EnvRender::none().gi.enabled, "the lux view would be relit");
    }

    /// An HDRI's sun must not reach the GPU as infinity.
    ///
    /// This is the bug that turned a loaded environment into a black rectangle wandering across the
    /// viewport. A stock Poly Haven sky peaks at 75360 against half-float's 65504 ceiling, so every
    /// sun texel arrived from the texture already `+Inf`. The bloom bright pass divides by the
    /// pixel's own brightness — `Inf / Inf` is NaN — and the pyramid's blur then spread that NaN
    /// across a whole mip level's worth of screen, which the driver writes as black. The
    /// power-of-two edges in the screenshots were the mip blocks.
    #[test]
    fn an_hdri_sun_survives_the_trip_to_the_gpu() {
        // The exact peak from the environment that broke it.
        assert!(75360.0 > HALF_MAX, "the premise: this HDRI is brighter than half-float holds");
        for v in [75360.0f32, 1.0e9, f32::INFINITY, HALF_MAX * 2.0] {
            let out = half_safe(&v);
            assert!(out.is_finite(), "{v} still reaches the GPU as {out}");
            assert!(out <= HALF_MAX, "{v} clamped to {out}, still above the ceiling");
        }
        // NaN in a source EXR would propagate exactly as badly.
        assert_eq!(half_safe(&f32::NAN), 0.0);
        // Ordinary values are untouched — this must not dim a normal sky.
        for v in [0.0f32, 0.5, 1.0, 12.75, 60000.0] {
            assert_eq!(half_safe(&v), v, "a value inside range was altered");
        }
        // Both uploads go through it: the backdrop copy AND the roughness chain. The chain is the
        // one that feeds reflections, so a missed clamp there blackens every glossy surface.
        let src = include_str!("light3d.rs");
        // Assembled at runtime so this test's own source does not count as a third match.
        let needle = format!("flatten().map({})", "half_safe");
        assert_eq!(
            src.matches(&needle).count(),
            2,
            "both the backdrop and the prefiltered chain must be clamped on upload"
        );
    }

    /// …and bloom must survive an infinite pixel even so.
    ///
    /// The upload clamp fixes the environment, but an emissive material can blow the buffer just as
    /// well. The bright pass is where infinity becomes NaN, so that is where it has to be stopped —
    /// belt as well as braces, because the failure is silent and enormous.
    #[test]
    fn the_bright_pass_cannot_produce_a_nan() {
        let fs = BLOOM_PRE_FS.replace("BLOOM_CEILING", "4000.0");
        assert!(!fs.contains("BLOOM_CEILING"), "the ceiling is substituted");
        let clamp = fs.find("c = min(max(c, vec3(0.0)), vec3(4000.0));").expect("the clamp");
        let guard = fs.find("if (any(isnan(c))) c = vec3(0.0);").expect("the NaN guard");
        let div = fs.find("/ max(br, 1e-5)").expect("the division that makes NaN");
        assert!(clamp < div && guard < div, "the clamp must come BEFORE the division, not after");
        // The ceiling has to leave headroom: the downsample sums four taps before averaging.
        assert!(4000.0 * 4.0 < HALF_MAX, "the pyramid's own sums could overflow the ceiling");
    }

    /// The callback must not leave a narrowed scissor behind it.
    ///
    /// eframe clears the window at the start of the NEXT frame, before egui sets any clip rect, and
    /// `glClear` obeys the scissor box. A scissor left at the 3D panel means that clear wipes only
    /// that rectangle — the panel goes black and every pixel outside it keeps the previous frame's
    /// image. It looks like a black hole where the viewport is with a frozen sky around it, and the
    /// boundary walks about as panels resize, because what you are seeing is the union of several
    /// frames' rects.
    #[test]
    fn the_callback_leaves_the_scissor_wide_open() {
        let src = include_str!("light3d.rs");
        assert!(
            src.contains("fn release_scissor("),
            "there is no place that reopens the scissor"
        );
        // BOTH exits — the ordinary one and the converged re-present — must call it. The converged
        // path is the easy one to forget, because it returns early.
        let calls = src.matches("release_scissor(gl, screen_w, screen_h)").count();
        assert!(calls >= 2, "only {calls} of the render's exits reopen the scissor");
        // …and it must open to the WINDOW, not to the viewport it just drew.
        let f = src.find("fn release_scissor(").expect("the helper");
        assert!(
            src[f..f + 300].contains("gl.scissor(0, 0, screen_w.max(1), screen_h.max(1))"),
            "the scissor is reopened to something smaller than the window"
        );
    }

    /// The composite must clip to its OWN viewport rect, not to whatever egui left set.
    ///
    /// egui scissors a callback to its CLIP rect — the enclosing panel — which can be larger than
    /// the rect the 3D view was actually allocated inside it. Trusting that meant a quad that was
    /// even slightly wrong painted the scene over its neighbours, which is exactly what a stray
    /// rectangle of 3D across the rest of the window looks like.
    #[test]
    fn the_composite_clips_to_its_own_rect() {
        let src = include_str!("light3d.rs");
        let f = src.find("unsafe fn composite(").expect("the composite");
        let body = &src[f..f + 1200];
        assert!(body.contains("gl.scissor(rect.0, rect.1"), "the composite sets its own scissor");
        assert!(body.contains("gl.enable(glow::SCISSOR_TEST)"), "…and enables it itself");
    }

    /// The converged path must never present a buffer nothing has written.
    ///
    /// That path re-presents the accumulation buffer INSTEAD of drawing the scene, so an empty
    /// buffer does not show up as one bad frame — the viewport goes black and stays black, because
    /// the whole point of the path is that there is no next render to correct it.
    #[test]
    fn the_converged_path_needs_a_real_image_to_present() {
        let mut r = Scene3dRenderer::default();
        r.taa_max = 16;
        r.taa_n = 16;
        r.taa_valid = false;
        assert!(!(r.taa_max > 0 && r.taa_valid && r.taa_n >= r.taa_max), "would present an empty buffer");
        r.taa_valid = true;
        assert!(r.taa_max > 0 && r.taa_valid && r.taa_n >= r.taa_max, "…but a written one is fine");
        // And the guard is really the one the renderer uses.
        let src = include_str!("light3d.rs");
        assert!(
            src.contains("if taa_on && self.taa_valid && self.taa_n >= self.taa_max"),
            "the re-present path does not check that the buffer holds anything"
        );
    }

    /// A resolve that cannot run must turn accumulation OFF, not declare it finished.
    ///
    /// "Finished" sends the very next frame down the re-present path, which shows the buffer the
    /// resolve just failed to write. Turning it off falls back to drawing the scene every frame —
    /// exactly what the renderer did before accumulation existed.
    #[test]
    fn a_failed_resolve_falls_back_to_drawing_the_scene() {
        let src = include_str!("light3d.rs");
        let f = src.find("The resolve could not run").expect("the failure path");
        let body = &src[f..f + 1000];
        assert!(body.contains("self.taa_max = 0;"), "accumulation is switched off");
        assert!(body.contains("self.taa_valid = false;"), "…and the buffer is marked empty");
        assert!(!body.contains("self.taa_n = self.taa_max;"), "it must not fake convergence");
    }

    /// The offscreen post passes must run with egui's SCISSOR OFF.
    ///
    /// egui's scissor rect is the panel in SCREEN coordinates; bloom's pyramid and the accumulation
    /// buffer are their own targets starting at (0, 0). With the scissor left on, every pixel of
    /// those targets that falls outside the panel's screen position is simply never written — so a
    /// viewport anywhere but the bottom-left corner gets a partly-stale pyramid, and the bug moves
    /// as the user resizes their panels. This is a source-ORDER property with no runtime hook, so
    /// it is asserted against the source; the real code comes first in the file, before this test.
    #[test]
    fn the_post_passes_run_outside_egui_scissor() {
        let src = include_str!("light3d.rs");
        let bloom = src.find("let bloom_ready = self.bloom_pass(").expect("the bloom pass");
        let taa = src.find("let t = self.taa_resolve(").expect("the accumulation resolve");
        let scissor = src
            .find("gl.enable(glow::SCISSOR_TEST); // egui's scissor")
            .expect("the composite re-enables the scissor");
        assert!(bloom < scissor, "bloom renders its pyramid inside egui's scissor");
        assert!(taa < scissor, "the accumulator renders inside egui's scissor");
    }

    /// A fresh renderer must not claim to be converging — nothing has asked for accumulation yet,
    /// and a caller that repaints on this alone would spin at full speed forever.
    #[test]
    fn accumulation_is_off_until_it_is_asked_for() {
        let r = Scene3dRenderer::default();
        assert!(!r.taa_converging());
        assert_eq!(r.taa_progress(), (0, 0));
    }

    /// A frame that has NOT yet held still must not ask for repaints.
    ///
    /// This is what stops a scene whose draw list is rebuilt slightly differently every frame from
    /// driving itself: repaint → key changes → repaint, at full speed, on a viewport nobody is
    /// touching, with the loop powered by the very requests meant to end it.
    #[test]
    fn a_frame_that_never_holds_still_does_not_drive_repaints() {
        let mut r = Scene3dRenderer::default();
        r.taa_max = 16;
        r.taa_n = 1;
        r.taa_stable = false;
        assert!(!r.taa_converging(), "a frame still changing asked to be redrawn");
        r.taa_stable = true;
        assert!(r.taa_converging(), "a settled frame stopped refining early");
        r.taa_n = 16;
        assert!(!r.taa_converging(), "a converged frame kept asking for more");
    }
}

#[cfg(test)]
mod bloom_tests {
    use super::*;

    /// The bright pass must compose the image the SAME WAY the composite does.
    ///
    /// The composite adds the ambient MRT and multiplies it by AO before the view transform. If the
    /// bright pass read only the colour attachment, everything lit by the sky would be invisible to
    /// bloom — a window blazing with ambient would not glow, while a sunlit patch beside it would,
    /// for no reason a user could ever deduce.
    #[test]
    fn bloom_sees_the_same_image_the_composite_does() {
        for term in ["u_amb", "u_ao", "u_ao_on"] {
            assert!(BLOOM_PRE_FS.contains(term), "the bright pass reads {term}, as the composite does");
            assert!(BLIT_FS.contains(term), "…and the composite still reads it");
        }
        assert!(BLOOM_PRE_FS.contains("c += a * ao;"), "composed identically");
        assert!(BLIT_FS.contains("c.rgb + a * ao"), "composed identically");
    }

    /// Bloom is added to SCENE-REFERRED light, inside `apply_view`'s argument. Added afterwards it
    /// would lift display values toward white — a wash, not a glow — and no amount of tuning the
    /// threshold would fix it.
    #[test]
    fn bloom_is_added_before_the_view_transform() {
        let add = BLIT_FS.find("u_bloom_k").expect("the composite adds bloom");
        let view = BLIT_FS.find("apply_view(lit)").expect("…and then applies the view transform");
        assert!(add < view, "bloom goes in before the transform, not after it");
        // And it is skippable, so a scene with bloom off pays for no texture fetch.
        assert!(BLIT_FS.contains("if (u_bloom_k > 0.0)"), "zero bloom costs nothing");
    }

    /// The threshold has a soft KNEE. A hard cutoff makes a contour crawl visibly across a surface
    /// as the exposure changes, because pixels cross the threshold one at a time.
    #[test]
    fn the_bright_pass_has_a_soft_knee() {
        assert!(BLOOM_PRE_FS.contains("u_knee"), "there is a knee");
        assert!(BLOOM_PRE_FS.contains("soft * soft"), "…and it is quadratic, not linear");
        // The default is on but gentle: this is a lighting app, so sources should glow by default.
        let d = crate::color::ColorPipeline::default();
        assert!(d.bloom > 0.0 && d.bloom < 0.15, "on, gently: {}", d.bloom);
        assert_eq!(d.bloom_threshold, 1.0, "only brighter-than-white blooms by default");
        // …and the passthrough pipeline, which exists to change nothing, must not bloom.
        assert_eq!(crate::color::ColorPipeline::passthrough().bloom, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fragment shaders are assembled by string substitution and [`compile`] **panics** on a
    /// bad one — at first paint, with the window already up. A GL context is not available in a
    /// unit test, so this checks the failures that substitution actually causes: a placeholder left
    /// unreplaced, a helper used before it is pasted in, or a uniform the Rust side looks up by a
    /// name the source never declares.
    fn assembled() -> Vec<(&'static str, String)> {
        vec![
            // Through the SAME assembly helpers the renderer uses — a test that built the shader
            // slightly differently from `ensure_init` would be testing a shader nobody compiles.
            ("scene", assemble_scene_fs()),
            ("transp", assemble_transp_fs()),
            ("textured", assemble_tex_fs()),
            ("blit", BLIT_FS.replace("FOG_GLSL", &fog_glsl()).replace("VIEW_GLSL", crate::color::VIEW_GLSL)),
            ("sky", assemble_sky_fs()),
            ("ssao", SSAO_FS.to_string()),
            ("blur", BLUR_FS.to_string()),
        ]
    }

    #[test]
    fn every_shader_placeholder_is_substituted() {
        for (name, src) in assembled() {
            for token in [
                "SHADOW_GLSL", "SRGB_GLSL", "VIEW_GLSL", "SKY_GLSL", "ENV_BRDF_GLSL",
                "ALBEDO_OUT_GLSL", "FOG_GLSL", "CASCADE_MAX_GLSL", "BLOOM_CEILING",
            ] {
                assert!(!src.contains(token), "{name}: `{token}` left unreplaced — the shader would not compile");
            }
            assert!(src.starts_with("\n    #version 330 core") || src.contains("#version 330 core"), "{name}: no #version");
            let (open, close) = (src.matches('{').count(), src.matches('}').count());
            assert_eq!(open, close, "{name}: unbalanced braces ({open} vs {close})");
        }
    }

    /// Every pass that draws into the main framebuffer must write ALL THREE colour attachments.
    ///
    /// With three draw buffers bound, a shader that declares only `location = 0` leaves the others
    /// UNDEFINED — not zero. GLSL will happily compile it, so the failure never announces itself:
    /// it shows up as garbage in the ambient or albedo term, and only for whichever pass forgot,
    /// which makes it look like a bug in that one material rather than a missing output.
    #[test]
    fn every_geometry_pass_writes_all_three_render_targets() {
        for (name, src) in assembled() {
            // The post passes write to single-attachment targets and are exempt.
            if matches!(name, "blit" | "ssao" | "blur") {
                assert!(!src.contains("location=1"), "{name} draws to one target; it must not declare a second");
                continue;
            }
            assert!(src.contains("layout(location=0) out"), "{name}: no explicit attachment-0 output");
            assert!(src.contains("layout(location=1) out"), "{name}: does not write the ambient attachment");
            assert!(src.contains("amb_out ="), "{name}: declares the ambient output but never assigns it");
            assert!(src.contains("layout(location=2) out"), "{name}: does not write the albedo attachment");
            assert!(src.contains("alb_out ="), "{name}: declares the albedo output but never assigns it");
        }
    }

    /// …and every path THROUGH those shaders must assign the albedo, not just the common one.
    ///
    /// The early `return` for a UI swatch is the trap: it writes two of the three outputs and
    /// leaves on the spot, so grid lines and selection tints would carry undefined albedo into the
    /// bounce. Counting assignments against returns is crude, but it catches exactly that shape.
    #[test]
    fn every_early_return_assigns_the_albedo_first() {
        for (name, src) in assembled() {
            if matches!(name, "blit" | "ssao" | "blur") {
                continue;
            }
            let main = src.find("void main()").expect("a main");
            let body = &src[main..];
            // Each `return;` inside main must be preceded by an `alb_out =` since the last one.
            let mut cursor = 0usize;
            let mut assigned = 0usize;
            while let Some(r) = body[cursor..].find("return;") {
                let at = cursor + r;
                assigned += body[cursor..at].matches("alb_out =").count();
                assert!(
                    assigned > 0,
                    "{name}: a path returns from main without ever assigning alb_out"
                );
                cursor = at + 7;
            }
        }
    }

    /// Ambient occlusion must reach the picture through the AMBIENT buffer and nothing else. If it
    /// ever multiplies the composite as a whole, a crease in direct sunlight goes grey — the single
    /// most common way SSAO is wrong, and it looks "atmospheric" enough to survive review.
    #[test]
    fn occlusion_only_scales_the_ambient_term() {
        let blit = BLIT_FS.replace("VIEW_GLSL", crate::color::VIEW_GLSL);
        assert!(blit.contains("c.rgb + a * ao"), "the composite must add direct + ambient·AO:\n{blit}");
        for (name, src) in assembled() {
            // The composite applies it and the blur produces it; nothing else may touch it.
            if matches!(name, "blit" | "blur") {
                continue;
            }
            // As a whole token: `u_aomap` (a material's baked AO map) is a different thing and is
            // allowed anywhere.
            assert!(!src.contains("u_ao;") && !src.contains("u_ao,") && !src.contains("u_ao)"), "{name} must not sample the screen-space occlusion buffer itself");
        }
    }

    /// Every helper a shader calls must be pasted in ahead of its first use — GLSL has no forward
    /// declarations, so an out-of-order include is a compile error, not a link warning.
    #[test]
    fn shader_helpers_are_defined_before_use() {
        for (name, src) in assembled() {
            for func in ["srgb_to_lin", "shadow_lit", "apply_view", "d_ggx", "v_smith", "f_schlick", "sh_ambient", "sky_radiance", "env_sample", "world_at"] {
                let def = src.find(&format!("{func}(")).filter(|_| src.contains(&format!(" {func}(")));
                let Some(def_at) = src.find(&format!("vec3 {func}(")).or(src.find(&format!("float {func}("))) else {
                    // Not used by this shader at all is fine; used-but-undefined is not.
                    assert!(!src.contains(&format!("{func}(")), "{name}: calls `{func}` but never defines it");
                    continue;
                };
                let _ = def;
                let first_call = src[def_at + 1..].find(&format!("{func}(")).map(|i| i + def_at + 1);
                assert!(first_call.is_some(), "{name}: `{func}` defined but never called");
            }
        }
    }

    /// Uniform names are strings on both sides — a typo silently yields `None` and the value never
    /// reaches the GPU, which shows up as "the slider does nothing" rather than as an error.
    #[test]
    fn every_looked_up_uniform_exists_in_its_shader() {
        // A uniform may live in either stage — the lookup is against the LINKED program.
        let tex = format!("{TEX_VS}{}", assemble_tex_fs());
        // Every name the sky/IBL block declares, so `SkyUniforms::locate` cannot go looking for one
        // the surface shader does not have.
        let sky_names = ["u_perez", "u_perez_norm", "u_zenith_xy", "u_sky_scale", "u_sky_sun", "u_sky_ground", "u_sky_sun_col", "u_sky_on", "u_sh"];
        for u in [
            "u_mvp", "u_img", "u_cam", "u_reflect", "u_proc", "u_col_a", "u_col_b", "u_pscale", "u_detail",
            "u_prough", "u_pcontrast", "u_ramp", "u_model", "u_sun_on", "u_sun_dir", "u_sun_col", "u_emission",
            "u_hl", "u_hl_k", "u_nrm", "u_rough", "u_has_nrm", "u_has_rough", "u_rough_base",
            "u_shadow_on", "u_light_mvp", "u_shadow", "u_clay", "u_metallic", "u_ior",
            "u_coat", "u_coat_rough", "u_sheen", "u_sheen_tint",
        ]
        .into_iter()
        .chain(sky_names)
        {
            assert!(tex.contains(&format!("{u};")) || tex.contains(&format!("{u} ")) || tex.contains(&format!("{u}[")), "textured shader has no `{u}`");
        }
        let blit = BLIT_FS
            .replace("FOG_GLSL", &fog_glsl())
            .replace("VIEW_GLSL", crate::color::VIEW_GLSL);
        for u in [
            "u_tex", "u_view", "u_exposure", "u_look", "u_amb", "u_ao", "u_ao_on",
            "u_fog_depth", "u_fog_inv_vp", "u_fog_cam", "u_fog_col", "u_fog_density", "u_fog_base",
            "u_fog_falloff", "u_fog_on",
        ] {
            assert!(blit.contains(u), "blit shader has no `{u}`");
        }
        let scene = SCENE_FS.replace("SHADOW_GLSL", &shadow_glsl()).replace("SRGB_GLSL", crate::color::SRGB_GLSL);
        for u in ["u_mvp", "u_alpha", "u_model", "u_shadow_on", "u_light_mvp", "u_shadow", "u_linearize"] {
            assert!(scene.contains(u) || SCENE_VS.contains(u), "scene shader has no `{u}`");
        }
        let sky = format!("{BLIT_VS}{}", SKY_FS.replace("SKY_GLSL", crate::env::SKY_GLSL));
        for u in ["u_inv_vp", "u_cam"].into_iter().chain(sky_names) {
            assert!(sky.contains(u), "sky shader has no `{u}`");
        }
        for u in ["u_depth", "u_vp", "u_inv_vp", "u_cam", "u_radius", "u_strength"] {
            assert!(SSAO_FS.contains(u), "ssao shader has no `{u}`");
        }
        assert!(BLUR_FS.contains("u_ao"), "blur shader has no `u_ao`");
    }

    /// The clearcoat must reflect about the GEOMETRIC normal, and the base must lose what it took.
    ///
    /// Both are the whole point. Using the bumped normal would make a lacquered board's reflection
    /// ripple with the grain, which is a shiny bump map and not varnish at all. And a second
    /// specular lobe added on top of a finished BSDF without taking anything away creates light —
    /// the easiest way there is to break energy conservation while still producing a picture.
    #[test]
    fn the_clearcoat_is_smooth_over_the_grain_and_pays_for_itself() {
        let fs = assemble_tex_fs();
        let coat = fs.split("if (u_coat > 0.0)").nth(1).expect("the clearcoat block");
        assert!(coat.contains("dot(Ng, V)"), "the coat reflects about the GEOMETRIC normal");
        assert!(coat.contains("reflect(-V, Ng)"), "…including its environment lobe");
        assert!(!coat.contains("dot(N, Hc)"), "…and never about the bumped one");
        // The attenuation must come BEFORE the coat's own lobes, or it would dim the very
        // reflection it is adding instead of the material underneath.
        let loss = coat.find("direct *= (1.0 - loss)").expect("the base is attenuated");
        let add = coat.find("direct += vec3(d_ggx").expect("the coat's sun lobe");
        assert!(loss < add, "the coat dims itself instead of the base");
        assert!(coat.contains("ambient *= (1.0 - loss)"), "the ambient pays too, not just the direct");
    }

    /// Sheen must be a GRAZING term, and it must survive when the sun is not the light.
    ///
    /// A velvet curtain in a north-facing room is the single most likely place anyone would put
    /// one; if the effect only existed under direct sun that is exactly where it would vanish.
    #[test]
    fn sheen_grazes_and_survives_without_a_sun() {
        let fs = assemble_tex_fs();
        let sheen = fs.split("if (u_sheen > 0.0)").nth(1).expect("the sheen block");
        assert!(sheen.contains("pow(clamp(1.0 - max(dot(V, Hs)"), "a Fresnel-shaped rim, not a flat lift");
        assert!(sheen.contains("ambient += u_sheen_tint"), "…that also shows under sky light alone");
        assert!(sheen.contains("amb_irr"), "…using the same irradiance the diffuse ambient came from");
        // Both branches of the lighting must define what the shared block reads, or the studio
        // path would compile against an uninitialised direction.
        for v in ["Ldir =", "Lrad =", "shf =", "amb_irr ="] {
            assert!(fs.matches(v).count() >= 2, "`{v}` is not set by both the daylight and studio branches");
        }
    }

    /// The surface shader must NOT tone-map any more — that moved to the composite. Two transforms
    /// in series is the kind of thing that looks "nearly right" and quietly crushes every midtone.
    #[test]
    fn tone_mapping_happens_only_at_the_composite() {
        let tex = TEX_FS.replace("SHADOW_GLSL", &shadow_glsl()).replace("SRGB_GLSL", crate::color::SRGB_GLSL);
        assert!(!tex.contains("exp(-col)"), "the old `1 - exp(-x)` tone-map is still in the surface shader");
        assert!(!tex.contains("apply_view"), "the surface shader must not run the view transform");
        let blit = BLIT_FS.replace("VIEW_GLSL", crate::color::VIEW_GLSL);
        assert!(blit.contains("apply_view"), "the composite must run the view transform");
    }
}

