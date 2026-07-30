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

/// One 3D vertex: position (metres, Z-up) + baked RGB colour.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
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
}

impl Default for ProcParams {
    fn default() -> Self {
        Self { mode: 0, col_a: [0.5, 0.5, 0.5], col_b: [0.5, 0.5, 0.5], scale: [1.0, 1.0, 1.0], detail: 6.0, rough: 0.6, contrast: 1.0, ramp: [0.35, 0.65] }
    }
}

const CEILING: u32 = 2; // material id skipped in the viewer so we can look in
const FLOOR: u32 = 0;

const IDENTITY16: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Per-texture PBR maps (Texture Phase 2): tangent-space **normal** + **roughness** map indices (into
/// the app's texture list, so they upload through the normal texture cache), plus a scalar roughness
/// used when there's no map. A texture absent here / all-`None` renders exactly as before.
#[derive(Clone, Copy, Debug)]
pub struct PbrParams {
    pub normal_idx: Option<usize>,
    pub rough_idx: Option<usize>,
    pub roughness: f32,
}

impl Default for PbrParams {
    fn default() -> Self {
        Self { normal_idx: None, rough_idx: None, roughness: 0.5 }
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
                out.push(V3 { x: p.x, y: p.y, z: p.z, r: col[0], g: col[1], b: col[2] });
            }
        }
    }
    out
}

/// Append a small bright octahedron marking a luminaire at (x, y, z).
pub fn push_luminaire_marker(out: &mut Vec<V3>, x: f32, y: f32, z: f32, s: f32) {
    let c = [1.0, 0.86, 0.38];
    let v = |dx: f32, dy: f32, dz: f32| V3 { x: x + dx, y: y + dy, z: z + dz, r: c[0], g: c[1], b: c[2] };
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
const SHADOW_GLSL: &str = r#"
    uniform int u_shadow_on;
    uniform mat4 u_light_mvp;
    uniform sampler2D u_shadow;
    float shadow_lit(vec3 wpos) {
        vec4 lc = u_light_mvp * vec4(wpos, 1.0);
        vec3 p = lc.xyz / lc.w * 0.5 + 0.5;
        if (p.z > 1.0 || p.x < 0.0 || p.x > 1.0 || p.y < 0.0 || p.y > 1.0) return 1.0;
        float bias = 0.0022;
        vec2 tx = 1.0 / vec2(textureSize(u_shadow, 0));
        float lit = 0.0;
        for (int y = -1; y <= 1; y++)
            for (int x = -1; x <= 1; x++) {
                float d = texture(u_shadow, p.xy + vec2(x, y) * tx).r;
                lit += (p.z - bias > d) ? 0.0 : 1.0;
            }
        return lit / 9.0;
    }
"#;

const SCENE_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec3 a_col;
    uniform mat4 u_mvp;
    uniform mat4 u_model;   // world = u_model * a_pos (identity for the world-space scene batch)
    out vec3 v_col;
    out vec3 v_wpos;
    void main() { gl_Position = u_mvp * vec4(a_pos, 1.0); v_col = a_col; v_wpos = (u_model * vec4(a_pos, 1.0)).xyz; }
"#;

const SCENE_FS: &str = r#"
    #version 330 core
    in vec3 v_col;
    in vec3 v_wpos;
    out vec4 frag;
    // Per-PASS alpha: V3 carries no alpha channel, so translucency for the overlay
    // pass (selection shade / modifier ghosts) rides on this uniform. 1.0 for the
    // opaque solid + line passes.
    uniform float u_alpha;
    SHADOW_GLSL
    void main() {
        vec3 col = v_col;
        if (u_shadow_on == 1) { col *= mix(0.5, 1.0, shadow_lit(v_wpos)); }
        frag = vec4(col, u_alpha);
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
    out vec4 frag;
    void main() { frag = vec4(v_col, v_a); }
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
    out vec4 frag;
    uniform sampler2D u_img;
    uniform vec3 u_cam;       // camera world position
    uniform float u_reflect;  // 0 = matte, 1 = maximum glossy sheen
    // PROCEDURAL material (evaluated from world position when u_proc > 0).
    uniform int   u_proc;       // 0=image, 1=wood, 2=marble, 3=noise, 4=checker
    uniform vec3  u_col_a;      // ramp low / colour A
    uniform vec3  u_col_b;      // ramp high / colour B
    uniform vec3  u_pscale;    // anisotropic world scale (across, along, through the grain)
    uniform float u_detail;    // fbm octaves
    uniform float u_prough;    // fbm amplitude falloff
    uniform float u_pcontrast; // ramp hardness about the midpoint
    uniform vec2  u_ramp;      // ramp low/high positions
    // Daylight (sun) — when u_sun_on, light this surface by the real sun instead of the baked shade.
    uniform int   u_sun_on;
    uniform vec3  u_sun_dir;   // TO the sun
    uniform vec3  u_sun_col;   // warm direct term
    uniform vec3  u_sky_col;   // cool ambient fill from ABOVE
    uniform vec3  u_ground_col; // warm ground bounce from BELOW (hemispheric fake GI)
    // PBR maps (Texture Phase 2). Tangent-space normal + roughness, sampled per fragment; the TBN is
    // reconstructed from screen-space derivatives so NO per-vertex tangents are needed.
    uniform sampler2D u_nrm;
    uniform sampler2D u_rough;
    uniform int   u_has_nrm;
    uniform int   u_has_rough;
    uniform float u_rough_base; // scalar roughness when there's no map
    uniform int   u_clay;       // 1 = override albedo to flat clay grey (glass keeps its alpha)
    uniform int   u_hl;         // 1 = this draw's material is selected in the Materials Factory
    uniform float u_hl_k;       // highlight pulse strength 0..1 (app-driven)
    SHADOW_GLSL

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
    // Map a raw 0..1 field through the two-stop ramp + contrast, then between the two colours.
    vec3 proc_ramp(float val){
        float t = smoothstep(u_ramp.x, u_ramp.y, val);
        t = clamp((t - 0.5) * u_pcontrast + 0.5, 0.0, 1.0);
        return mix(u_col_a, u_col_b, t);
    }
    vec3 proc_color(){
        vec3 p = v_wpos * u_pscale;
        if (u_proc == 1) {          // wood — anisotropic fbm straight into the ramp
            return proc_ramp(fbm(p));
        } else if (u_proc == 2) {   // marble — fbm turbulence folded through a sine band
            float band = 0.5 + 0.5 * sin((p.x + p.y) * 0.6 + fbm(p) * 6.2831);
            return proc_ramp(band);
        } else if (u_proc == 4) {   // checker — hard cells in world space
            vec3 c = floor(p);
            float parity = mod(c.x + c.y + c.z, 2.0);
            return mix(u_col_a, u_col_b, parity);
        }
        return proc_ramp(fbm(p));   // 3 = plain noise
    }

    // Output alpha = the vertex opacity × the image's own alpha channel. Opaque draws run with
    // blending OFF, so this alpha is simply ignored there; only the blended textured pass uses it.
    void main() {
        vec2 uv = v_uv;
        vec4 t = u_proc > 0 ? vec4(proc_color(), 1.0) : texture(u_img, uv);
        vec3 albedo = u_clay == 1 ? vec3(0.62) : t.rgb; // clay: flat grey, keep t.a for glass

        // Geometric normal from screen-space derivatives (no per-vertex normal needed), faced to
        // the viewer so two-sided surfaces light correctly.
        vec3 Ng = normalize(cross(dFdx(v_wpos), dFdy(v_wpos)));
        vec3 V = normalize(u_cam - v_wpos);
        if (dot(Ng, V) < 0.0) Ng = -Ng;
        vec3 N = Ng;

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
        float rough = (u_has_rough == 1) ? texture(u_rough, uv).r : u_rough_base;

        vec3 col;
        if (u_sun_on == 1) {
            // Physically-flavoured daylight: cool sky fill + warm direct sun (shadowed) + a
            // roughness-controlled specular highlight. u_sky_col / u_sun_col are HDR (can exceed 1).
            float sh = (u_shadow_on == 1) ? shadow_lit(v_wpos) : 1.0;
            float ndl = max(dot(N, u_sun_dir), 0.0);
            // Hemispheric ambient: sky from above, warm ground bounce from below (fake GI).
            vec3 amb = mix(u_ground_col, u_sky_col, clamp(0.5 + 0.5 * N.z, 0.0, 1.0));
            col = albedo * (amb + u_sun_col * ndl * sh);
            vec3 H = normalize(u_sun_dir + V);
            float shin = mix(6.0, 220.0, clamp(1.0 - rough, 0.0, 1.0));
            float spec = pow(max(dot(N, H), 0.0), shin) * (1.0 - rough) * 0.7 * sh;
            col += u_sun_col * spec;
            // Exposure tone-map (matches factory::tonemap): roll HDR highlights off smoothly and lift
            // the shadow side into daylight instead of clipping to flat white / crushing to dusk.
            col = vec3(1.0) - exp(-col);
        } else {
            col = albedo * v_shade; // the baked studio scalar (sun off)
        }

        // Legacy view-dependent glossy sheen (kept; independent of the sun).
        if (u_reflect > 0.001) {
            float fres = pow(1.0 - clamp(abs(dot(N, V)), 0.0, 1.0), 3.0);
            float k = u_reflect * (0.15 + 0.85 * fres);
            vec3 sheen = mix(vec3(0.35), vec3(1.0), fres);
            col = mix(col, sheen, clamp(k, 0.0, 1.0));
        }
        // MATERIAL HIGHLIGHT — the material selected in the Materials Factory pulses cyan so the
        // user sees exactly WHERE it is while tuning it. u_hl flags this draw's texture; u_hl_k is
        // the app-driven pulse (0..1). Applied after everything so it reads on dark surfaces too.
        if (u_hl == 1) {
            col = mix(col, vec3(0.25, 0.85, 1.0), u_hl_k * 0.45);
        }
        frag = vec4(col, t.a * v_a);
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

const BLIT_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    void main() { frag = texture(u_tex, v_uv); }
"#;

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
    u_tex_sky_col: Option<glow::UniformLocation>,
    u_tex_ground_col: Option<glow::UniformLocation>,
    u_tex_hl: Option<glow::UniformLocation>,
    u_tex_hl_k: Option<glow::UniformLocation>,
    u_tex_nrm: Option<glow::UniformLocation>,
    u_tex_rough: Option<glow::UniformLocation>,
    u_tex_has_nrm: Option<glow::UniformLocation>,
    u_tex_has_rough: Option<glow::UniformLocation>,
    u_tex_rough_base: Option<glow::UniformLocation>,
    u_tex_shadow_on: Option<glow::UniformLocation>,
    u_tex_light_mvp: Option<glow::UniformLocation>,
    u_tex_shadow: Option<glow::UniformLocation>,
    u_tex_clay: Option<glow::UniformLocation>,
    // Scene program: world-model + shadow uniforms.
    u_scene_model: Option<glow::UniformLocation>,
    u_scene_shadow_on: Option<glow::UniformLocation>,
    u_scene_light_mvp: Option<glow::UniformLocation>,
    u_scene_shadow: Option<glow::UniformLocation>,
    // Depth (shadow-map) program + its FBO/texture.
    depth_prog: Option<glow::Program>,
    u_depth_mvp: Option<glow::UniformLocation>,
    shadow_fbo: Option<glow::Framebuffer>,
    shadow_tex: Option<glow::Texture>,
    shadow_size: i32,
    tex_bufs: std::collections::HashMap<u64, (glow::VertexArray, glow::Buffer, i32)>,
    tex_images: std::collections::HashMap<usize, glow::Texture>,
    // Shared DYNAMIC buffer for textured FEATURES (walls/floors), whose world-space geometry
    // changes on every recompute — re-uploaded each frame instead of cached per key.
    tex_dyn_vao: Option<glow::VertexArray>,
    tex_dyn_vbo: Option<glow::Buffer>,
    blit_prog: Option<glow::Program>,
    u_tex: Option<glow::UniformLocation>,
    blit_vao: Option<glow::VertexArray>,
    blit_vbo: Option<glow::Buffer>,
    fbo: Option<glow::Framebuffer>,
    color: Option<glow::Texture>,
    depth: Option<glow::Renderbuffer>,
    fbo_w: i32,
    fbo_h: i32,
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
            u_tex_sky_col: None,
            u_tex_ground_col: None,
            u_tex_hl: None,
            u_tex_hl_k: None,
            u_tex_nrm: None,
            u_tex_rough: None,
            u_tex_has_nrm: None,
            u_tex_has_rough: None,
            u_tex_rough_base: None,
            u_tex_shadow_on: None,
            u_tex_light_mvp: None,
            u_tex_shadow: None,
            u_tex_clay: None,
            u_scene_model: None,
            u_scene_shadow_on: None,
            u_scene_light_mvp: None,
            u_scene_shadow: None,
            depth_prog: None,
            u_depth_mvp: None,
            shadow_fbo: None,
            shadow_tex: None,
            shadow_size: 2048,
            tex_bufs: std::collections::HashMap::new(),
            tex_images: std::collections::HashMap::new(),
            tex_dyn_vao: None,
            tex_dyn_vbo: None,
            blit_prog: None,
            u_tex: None,
            blit_vao: None,
            blit_vbo: None,
            fbo: None,
            color: None,
            depth: None,
            fbo_w: 0,
            fbo_h: 0,
        }
    }
}

impl Scene3dRenderer {
    fn ensure_init(&mut self, gl: &glow::Context) {
        if self.inited {
            return;
        }
        unsafe {
            // --- scene program + VAO (position + colour, interleaved) ---
            let scene_fs = SCENE_FS.replace("SHADOW_GLSL", SHADOW_GLSL);
            let scene_prog = compile(gl, SCENE_VS, &scene_fs);
            self.u_mvp = gl.get_uniform_location(scene_prog, "u_mvp");
            self.u_alpha = gl.get_uniform_location(scene_prog, "u_alpha");
            self.u_scene_model = gl.get_uniform_location(scene_prog, "u_model");
            self.u_scene_shadow_on = gl.get_uniform_location(scene_prog, "u_shadow_on");
            self.u_scene_light_mvp = gl.get_uniform_location(scene_prog, "u_light_mvp");
            self.u_scene_shadow = gl.get_uniform_location(scene_prog, "u_shadow");
            // --- depth (shadow map) program ---
            let depth_prog = compile(gl, DEPTH_VS, DEPTH_FS);
            self.u_depth_mvp = gl.get_uniform_location(depth_prog, "u_depth_mvp");
            self.depth_prog = Some(depth_prog);
            let svbo = gl.create_buffer().unwrap();
            let svao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(svao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(svbo));
            let stride = size_of::<V3>() as i32; // 24
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);

            // --- two STATIC scene VAOs/VBOs (same pos+colour layout) ---
            for i in 0..2 {
                let vbo = gl.create_buffer().unwrap();
                let vao = gl.create_vertex_array().unwrap();
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
                self.static_vao[i] = Some(vao);
                self.static_vbo[i] = Some(vbo);
            }

            // --- transparent furniture program (pos+colour+opacity); VAOs built lazily ---
            let transp_prog = compile(gl, TRANSP_VS, TRANSP_FS);
            self.u_transp_mvp = gl.get_uniform_location(transp_prog, "u_mvp");
            self.transp_prog = Some(transp_prog);

            // --- textured program (pos+uv+shade); per-mesh VAOs are built lazily ---
            let tex_fs = TEX_FS.replace("SHADOW_GLSL", SHADOW_GLSL);
            let tex_prog = compile(gl, TEX_VS, &tex_fs);
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
            self.u_tex_sky_col = gl.get_uniform_location(tex_prog, "u_sky_col");
            self.u_tex_ground_col = gl.get_uniform_location(tex_prog, "u_ground_col");
            self.u_tex_hl = gl.get_uniform_location(tex_prog, "u_hl");
            self.u_tex_hl_k = gl.get_uniform_location(tex_prog, "u_hl_k");
            self.u_tex_nrm = gl.get_uniform_location(tex_prog, "u_nrm");
            self.u_tex_rough = gl.get_uniform_location(tex_prog, "u_rough");
            self.u_tex_has_nrm = gl.get_uniform_location(tex_prog, "u_has_nrm");
            self.u_tex_has_rough = gl.get_uniform_location(tex_prog, "u_has_rough");
            self.u_tex_rough_base = gl.get_uniform_location(tex_prog, "u_rough_base");
            self.u_tex_shadow_on = gl.get_uniform_location(tex_prog, "u_shadow_on");
            self.u_tex_light_mvp = gl.get_uniform_location(tex_prog, "u_light_mvp");
            self.u_tex_shadow = gl.get_uniform_location(tex_prog, "u_shadow");
            self.u_tex_clay = gl.get_uniform_location(tex_prog, "u_clay");
            self.tex_prog = Some(tex_prog);
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
            let blit_prog = compile(gl, BLIT_VS, BLIT_FS);
            self.u_tex = gl.get_uniform_location(blit_prog, "u_tex");
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

            self.scene_prog = Some(scene_prog);
            self.scene_vao = Some(svao);
            self.scene_vbo = Some(svbo);
            self.blit_prog = Some(blit_prog);
            self.blit_vao = Some(bvao);
            self.blit_vbo = Some(bvbo);
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
        if let Some(t) = self.color.take() {
            gl.delete_texture(t);
        }
        if let Some(d) = self.depth.take() {
            gl.delete_renderbuffer(d);
        }

        let color = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(color));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

        let depth = gl.create_renderbuffer().unwrap();
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);

        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color), 0);
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_renderbuffer(glow::RENDERBUFFER, None);

        self.fbo = Some(fbo);
        self.color = Some(color);
        self.depth = Some(depth);
        self.fbo_w = w;
        self.fbo_h = h;
    }

    /// Lazily create the sun's shadow-map FBO: a single depth texture the sun's depth pass renders
    /// into and the main passes sample. Bordered so anything outside the light frustum reads "lit".
    unsafe fn ensure_shadow_fbo(&mut self, gl: &glow::Context) {
        if self.shadow_fbo.is_some() {
            return;
        }
        let s = self.shadow_size;
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::DEPTH_COMPONENT24 as i32, s, s, 0,
            glow::DEPTH_COMPONENT, glow::FLOAT, glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_BORDER as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_BORDER as i32);
        gl.tex_parameter_f32_slice(glow::TEXTURE_2D, glow::TEXTURE_BORDER_COLOR, &[1.0, 1.0, 1.0, 1.0]);

        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, Some(tex), 0);
        gl.draw_buffer(glow::NONE);
        gl.read_buffer(glow::NONE);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
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
        let stride = size_of::<V3>() as i32;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
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

    /// Ensure the app-side texture `idx` (RGBA8, `w`×`h`, top row first) is uploaded to a GL
    /// texture, uploading it once and caching by index. Returns the GL handle.
    unsafe fn ensure_texture(&mut self, gl: &glow::Context, idx: usize, w: i32, h: i32, rgba: &[u8]) -> Option<glow::Texture> {
        if let Some(t) = self.tex_images.get(&idx) {
            return Some(*t);
        }
        if w <= 0 || h <= 0 || rgba.len() < (w as usize * h as usize * 4) {
            return None;
        }
        let tex = gl.create_texture().ok()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0,
            glow::RGBA, glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&rgba[..(w as usize * h as usize * 4)])),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.tex_images.insert(idx, tex);
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
    }

    /// Bind the PBR normal/roughness maps (units 2/3) for the textured program and set their
    /// presence flags + scalar roughness. Maps come from the app's texture cache (`tex_images`).
    unsafe fn set_pbr(&self, gl: &glow::Context, pbr: &PbrParams) {
        let nrm = pbr.normal_idx.and_then(|i| self.tex_images.get(&i).copied());
        let rgh = pbr.rough_idx.and_then(|i| self.tex_images.get(&i).copied());
        if let Some(loc) = &self.u_tex_has_nrm { gl.uniform_1_i32(Some(loc), nrm.is_some() as i32); }
        if let Some(loc) = &self.u_tex_has_rough { gl.uniform_1_i32(Some(loc), rgh.is_some() as i32); }
        if let Some(loc) = &self.u_tex_rough_base { gl.uniform_1_f32(Some(loc), pbr.roughness); }
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
        // DAYLIGHT: `Some((dir, sun_rgb, sky_rgb, ground_rgb))` lights every surface by the sun (dir
        // points TO the sun) with a hemispheric sky/ground ambient, instead of the baked studio
        // scalar. `None` = the fixed studio light (unchanged).
        sun: Option<([f32; 3], [f32; 3], [f32; 3], [f32; 3])>,
        // Sun SHADOWS: `Some(light_mvp)` renders a depth pass from the sun and shadows the scene.
        // `None` (or `sun` None) = no cast shadows. Only meaningful together with `sun`.
        shadow_mvp: Option<[f32; 16]>,
        // CLAY mode: force textured/procedural surfaces to flat grey (glass keeps its alpha). The
        // flat V3 scene/furniture are greyed CPU-side (see `factory::set_clay`); this covers the
        // textured pass, which samples its albedo in-shader.
        clay: bool,
        // MATERIAL HIGHLIGHT: `Some((texture_index, pulse 0..1))` tints every surface using that
        // material cyan (pulsing) so the Materials Factory selection is visible in the scene.
        highlight: Option<(usize, f32)>,
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
        unsafe {
            self.ensure_fbo(gl, vp_w, vp_h);

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
                self.tex_images.retain(|k, &mut tex| {
                    if live_img.contains(k) { return true; }
                    gl.delete_texture(tex);
                    false
                });
            }

            // ---- SUN SHADOW MAP: depth pass from the sun's point of view ----
            // Only when daylight + shadows are requested. Renders the opaque casters (scene +
            // furniture) into the depth-only shadow FBO with the light-space matrix; the main
            // passes below sample it. Front-face culling during the depth pass curbs shadow acne.
            let do_shadow = sun.is_some() && shadow_mvp.is_some();
            if do_shadow {
                let lmvp = shadow_mvp.unwrap();
                self.ensure_shadow_fbo(gl);
                if let (Some(dprog), Some(sfbo)) = (self.depth_prog, self.shadow_fbo) {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sfbo));
                    gl.viewport(0, 0, self.shadow_size, self.shadow_size);
                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_func(glow::LESS);
                    gl.disable(glow::BLEND);
                    gl.clear(glow::DEPTH_BUFFER_BIT);
                    gl.enable(glow::CULL_FACE);
                    gl.cull_face(glow::FRONT);
                    gl.use_program(Some(dprog));
                    // Scene (world-space) → depth_mvp = light_mvp. Upload into the slot-0 static
                    // buffer here if stale so the main opaque draw below reuses it.
                    if !verts.is_empty() {
                        if let (Some(v), Some(vao), Some(vbo)) = (scene_ver, self.static_vao[0], self.static_vbo[0]) {
                            if self.static_ver[0] != v {
                                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::STATIC_DRAW);
                                self.static_ver[0] = v;
                                self.static_len[0] = verts.len() as i32;
                            }
                            if let Some(loc) = &self.u_depth_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &lmvp); }
                            gl.bind_vertex_array(Some(vao));
                            gl.draw_arrays(glow::TRIANGLES, 0, self.static_len[0]);
                        }
                    }
                    // Furniture (local) → depth_mvp = light_mvp · model.
                    let lm = Mat4::from_cols_array(&lmvp);
                    for &(key, fverts, _fmvp, ref model) in furn {
                        if let Some((vao, count)) = self.furn_buf(gl, key, fverts) {
                            let dm = (lm * Mat4::from_cols_array(model)).to_cols_array();
                            if let Some(loc) = &self.u_depth_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &dm); }
                            gl.bind_vertex_array(Some(vao));
                            gl.draw_arrays(glow::TRIANGLES, 0, count);
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
                gl.bind_texture(glow::TEXTURE_2D, self.shadow_tex);
                gl.active_texture(glow::TEXTURE0);
            }
            let lmvp = shadow_mvp.unwrap_or(IDENTITY16);
            if let Some(prog) = self.scene_prog {
                gl.use_program(Some(prog));
                if let Some(loc) = &self.u_scene_shadow_on { gl.uniform_1_i32(Some(loc), do_shadow as i32); }
                if let Some(loc) = &self.u_scene_light_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &lmvp); }
                if let Some(loc) = &self.u_scene_shadow { gl.uniform_1_i32(Some(loc), 1); }
            }
            if let Some(prog) = self.tex_prog {
                gl.use_program(Some(prog));
                let on = sun.is_some();
                if let Some(loc) = &self.u_tex_sun_on { gl.uniform_1_i32(Some(loc), on as i32); }
                if let Some((d, sc, sk, gr)) = sun {
                    if let Some(loc) = &self.u_tex_sun_dir { gl.uniform_3_f32(Some(loc), d[0], d[1], d[2]); }
                    if let Some(loc) = &self.u_tex_sun_col { gl.uniform_3_f32(Some(loc), sc[0], sc[1], sc[2]); }
                    if let Some(loc) = &self.u_tex_sky_col { gl.uniform_3_f32(Some(loc), sk[0], sk[1], sk[2]); }
                    if let Some(loc) = &self.u_tex_ground_col { gl.uniform_3_f32(Some(loc), gr[0], gr[1], gr[2]); }
                }
                if let Some(loc) = &self.u_tex_shadow_on { gl.uniform_1_i32(Some(loc), do_shadow as i32); }
                if let Some(loc) = &self.u_tex_light_mvp { gl.uniform_matrix_4_f32_slice(Some(loc), false, &lmvp); }
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
            gl.clear_color(0.07, 0.086, 0.11, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

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
                for &(idx, w, h, rgba) in tex_assets {
                    let _ = self.ensure_texture(gl, idx, w, h, rgba);
                }
                // Furniture (persistent per-key buffers + model matrix).
                for &(tex_idx, mesh_key, verts, ref tmvp, ref model) in tex_draws {
                    if let Some(img) = self.tex_images.get(&tex_idx).copied() {
                        self.draw_textured(gl, mesh_key, verts, tmvp, model, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
                // Feature surfaces (dynamic, world-space, scene mvp).
                for &(tex_idx, verts) in tex_feat {
                    if let Some(img) = self.tex_images.get(&tex_idx).copied() {
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
                    if let Some(img) = self.tex_images.get(&tex_idx).copied() {
                        self.draw_textured(gl, mesh_key, verts, tmvp, model, img, cam_pos, reflect_of(tex_idx), proc_of(tex_idx), pbr_of(tex_idx), hl_of(tex_idx));
                    }
                }
                // See-through CSG feature solids (world-space, scene mvp); caller sorts back-to-front.
                for &(tex_idx, verts) in tex_feat_transp {
                    if let Some(img) = self.tex_images.get(&tex_idx).copied() {
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

            // ---- composite the colour texture into the panel rect --------
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, screen_w.max(1), screen_h.max(1)); // restore egui's full-screen viewport
            gl.enable(glow::SCISSOR_TEST); // egui's scissor (= this rect) is still set
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);

            let sw = screen_w.max(1) as f32;
            let sh = screen_h.max(1) as f32;
            let x0 = 2.0 * vp_left as f32 / sw - 1.0;
            let x1 = 2.0 * (vp_left + vp_w) as f32 / sw - 1.0;
            let y0 = 2.0 * vp_from_bottom as f32 / sh - 1.0;
            let y1 = 2.0 * (vp_from_bottom + vp_h) as f32 / sh - 1.0;
            // 6 verts: pos.xy, uv
            let quad: [f32; 24] = [
                x0, y0, 0.0, 0.0,  x1, y0, 1.0, 0.0,  x1, y1, 1.0, 1.0,
                x0, y0, 0.0, 0.0,  x1, y1, 1.0, 1.0,  x0, y1, 0.0, 1.0,
            ];
            if let (Some(prog), Some(vao), Some(vbo), Some(color)) =
                (self.blit_prog, self.blit_vao, self.blit_vbo, self.color)
            {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&quad), glow::DYNAMIC_DRAW);
                gl.use_program(Some(prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(color));
                if let Some(loc) = &self.u_tex {
                    gl.uniform_1_i32(Some(loc), 0);
                }
                gl.bind_vertex_array(Some(vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }

            // Leave egui's expected state: blend on, program unbound.
            gl.enable(glow::BLEND);
            gl.use_program(None);
        }
    }

}

unsafe fn compile(gl: &glow::Context, vs_src: &str, fs_src: &str) -> glow::Program {
    let program = gl.create_program().expect("create_program");
    let compile_one = |src: &str, kind: u32| -> glow::Shader {
        let s = gl.create_shader(kind).expect("create_shader");
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            panic!("SIMLUX 3D shader compile failed:\n{}", gl.get_shader_info_log(s));
        }
        s
    };
    let vs = compile_one(vs_src, glow::VERTEX_SHADER);
    let fs = compile_one(fs_src, glow::FRAGMENT_SHADER);
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("SIMLUX 3D program link failed:\n{}", gl.get_program_info_log(program));
    }
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    program
}

/// Reinterpret a `&[T]` of `Copy` POD as bytes, for `glBufferData`.
fn bytes<T: Copy>(slice: &[T]) -> &[u8] {
    let len = std::mem::size_of_val(slice);
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, len) }
}
