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

const CEILING: u32 = 2; // material id skipped in the viewer so we can look in
const FLOOR: u32 = 0;

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
        Mat4::perspective_rh_gl(45f32.to_radians(), aspect.max(0.01), 0.05, (dist * 6.0).max(80.0))
    };
    (proj * view).to_cols_array()
}

const SCENE_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec3 a_col;
    uniform mat4 u_mvp;
    out vec3 v_col;
    void main() { gl_Position = u_mvp * vec4(a_pos, 1.0); v_col = a_col; }
"#;

const SCENE_FS: &str = r#"
    #version 330 core
    in vec3 v_col;
    out vec4 frag;
    // Per-PASS alpha: V3 carries no alpha channel, so translucency for the overlay
    // pass (selection shade / modifier ghosts) rides on this uniform. 1.0 for the
    // opaque solid + line passes.
    uniform float u_alpha;
    void main() { frag = vec4(v_col, u_alpha); }
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
            let scene_prog = compile(gl, SCENE_VS, SCENE_FS);
            self.u_mvp = gl.get_uniform_location(scene_prog, "u_mvp");
            self.u_alpha = gl.get_uniform_location(scene_prog, "u_alpha");
            let svbo = gl.create_buffer().unwrap();
            let svao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(svao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(svbo));
            let stride = size_of::<V3>() as i32; // 24
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);

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
    pub fn render(
        &mut self,
        gl: &glow::Context,
        verts: &[V3],
        overlay: &[V3],
        lines: &[V3],
        mvp: &[f32; 16],
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
                if let (Some(prog), Some(vao), Some(vbo)) = (self.scene_prog, self.scene_vao, self.scene_vbo) {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(verts), glow::DYNAMIC_DRAW);
                    gl.use_program(Some(prog));
                    if let Some(loc) = &self.u_mvp {
                        gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp);
                    }
                    if let Some(loc) = &self.u_alpha {
                        gl.uniform_1_f32(Some(loc), 1.0); // opaque
                    }
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, verts.len() as i32);
                    gl.bind_vertex_array(None);
                }
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
