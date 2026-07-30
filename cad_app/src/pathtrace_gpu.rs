//! GPU path-tracer backend — the same render core as [`crate::pathtrace`], traced in a **GL 3.3
//! fragment shader** (no compute, no RT cores needed — runs on the app's existing glow context).
//!
//! The scene ([`crate::pathtrace::GpuPack`]) is packed into RGBA32F textures fetched with
//! `texelFetch`; each pass draws one fullscreen triangle that traces 1 sample/pixel and ADDS it
//! into an RGBA32F accumulation FBO (`glBlendFunc(ONE, ONE)`), so the image refines progressively
//! exactly like the CPU backend. The GLSL below is a line-for-line port of `pathtrace::trace` —
//! same thin-glass model, same sun NEE with glass-aware shadow transmission, same hemispheric sky,
//! so CPU and GPU converge to the same image.
//!
//! Driven from the UI thread (GL is single-threaded): the Render dialog calls [`GpuTracer::step`]
//! with a small pass batch each frame, and [`GpuTracer::snapshot`] reads the accumulation back only
//! when the preview refreshes (readback stalls the pipeline, so not every pass).

use crate::pathtrace::{tonemap8, Camera, GpuPack, Settings, Sky};
use eframe::glow::{self, HasContext}; // eframe's glow (the app's real GL context type)

/// Texture row width for the texel streams (2^11 — the shader indexes with `& 2047` / `>> 11`).
const TEXW: usize = 2048;

const VS: &str = r#"
    #version 330 core
    const vec2 P[3] = vec2[3](vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    void main() { gl_Position = vec4(P[gl_VertexID], 0.0, 1.0); }
"#;

const FS: &str = r#"
    #version 330 core
    out vec4 frag;

    uniform sampler2D u_tris;   // 5 texels/tri: [p0|rough][e1|metal][e2|opacity][albedo|0][emission|0]
    uniform sampler2D u_nodes;  // 2 texels/node: [mn|right_or_start][mx|count]
    uniform sampler2D u_order;  // 1 texel/entry: [tri_index|0|0|0]
    uniform vec3 u_eye;
    uniform vec3 u_fwd;
    uniform vec3 u_right;
    uniform vec3 u_up;
    uniform float u_half;       // tan(fov/2)
    uniform float u_aspect;
    uniform vec2 u_res;
    uniform vec3 u_sun_dir;
    uniform vec3 u_sun_col;
    uniform vec3 u_sky_col;
    uniform vec3 u_ground_col;
    uniform uint u_pass;

    // ---- RNG (pcg-ish) ----
    uint g_state;
    float rnd() {
        g_state = g_state * 747796405u + 2891336453u;
        uint w = ((g_state >> ((g_state >> 28u) + 4u)) ^ g_state) * 277803737u;
        w = (w >> 22u) ^ w;
        return float(w & 16777215u) / 16777216.0;
    }

    vec4 fetchT(sampler2D t, int i) { return texelFetch(t, ivec2(i & 2047, i >> 11), 0); }

    // ---- ray/tri + ray/box ----
    float rayTri(vec3 ro, vec3 rd, vec3 p0, vec3 e1, vec3 e2) {
        vec3 p = cross(rd, e2);
        float det = dot(e1, p);
        if (abs(det) < 1e-9) return -1.0;
        float inv = 1.0 / det;
        vec3 s = ro - p0;
        float u = dot(s, p) * inv;
        if (u < 0.0 || u > 1.0) return -1.0;
        vec3 q = cross(s, e1);
        float v = dot(rd, q) * inv;
        if (v < 0.0 || u + v > 1.0) return -1.0;
        float t = dot(e2, q) * inv;
        return (t > 1e-4) ? t : -1.0;
    }
    bool rayBox(vec3 ro, vec3 inv, vec3 mn, vec3 mx, float tmax) {
        vec3 t0 = (mn - ro) * inv;
        vec3 t1 = (mx - ro) * inv;
        vec3 tsm = min(t0, t1), tbg = max(t0, t1);
        float near = max(max(tsm.x, tsm.y), max(tsm.z, 0.0));
        float far = min(min(tbg.x, tbg.y), min(tbg.z, tmax));
        return near <= far;
    }

    // ---- BVH traversal: nearest hit (returns tri index or -1; t in outT) ----
    int intersect(vec3 ro, vec3 rd, out float outT) {
        vec3 inv = 1.0 / rd;
        int stack[48];
        int sp = 0;
        stack[sp++] = 0;
        float tmax = 1e30;
        int best = -1;
        while (sp > 0) {
            int ni = stack[--sp];
            vec4 n0 = fetchT(u_nodes, ni * 2);
            vec4 n1 = fetchT(u_nodes, ni * 2 + 1);
            if (!rayBox(ro, inv, n0.xyz, n1.xyz, tmax)) continue;
            int count = int(n1.w);
            if (count > 0) {
                int start = int(n0.w);
                for (int k = start; k < start + count; k++) {
                    int ti = int(fetchT(u_order, k).x);
                    int b = ti * 5;
                    float t = rayTri(ro, rd, fetchT(u_tris, b).xyz, fetchT(u_tris, b + 1).xyz, fetchT(u_tris, b + 2).xyz);
                    if (t > 0.0 && t < tmax) { tmax = t; best = ti; }
                }
            } else if (sp + 2 <= 48) {
                stack[sp++] = ni + 1;
                stack[sp++] = int(n0.w);
            }
        }
        outT = tmax;
        return best;
    }

    // ---- sun shadow transmission: glass attenuates, opaque blocks ----
    vec3 transmission(vec3 ro, vec3 rd) {
        vec3 tr = vec3(1.0);
        for (int i = 0; i < 16; i++) {
            float t;
            int ti = intersect(ro, rd, t);
            if (ti < 0) return tr;
            int b = ti * 5;
            float opacity = fetchT(u_tris, b + 2).w;
            if (opacity >= 0.99) return vec3(0.0);
            vec3 alb = fetchT(u_tris, b + 3).xyz;
            tr *= (1.0 - opacity) * (0.5 + 0.5 * alb);
            if (max(tr.r, max(tr.g, tr.b)) < 0.01) return vec3(0.0);
            ro = ro + rd * (t + 1e-3);
        }
        return tr;
    }

    vec3 cosineDir(vec3 n) {
        float r1 = rnd() * 6.2831853;
        float r2 = rnd();
        float r2s = sqrt(r2);
        vec3 a = (abs(n.x) > 0.5) ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
        vec3 u = normalize(cross(a, n));
        vec3 v = cross(n, u);
        return normalize(u * (cos(r1) * r2s) + v * (sin(r1) * r2s) + n * sqrt(1.0 - r2));
    }

    vec3 skyRadiance(vec3 dir, bool primary) {
        float up = clamp(0.5 + 0.5 * dir.z, 0.0, 1.0);
        vec3 c = mix(u_ground_col, u_sky_col, up);
        if (primary) {
            float d = dot(dir, u_sun_dir);
            if (d > 0.9995) c += u_sun_col * clamp((d - 0.9995) / 0.0005, 0.0, 1.0) * 2.0;
        }
        return c;
    }

    vec3 trace(vec3 ro, vec3 rd) {
        vec3 radiance = vec3(0.0);
        vec3 through = vec3(1.0);
        bool primary = true;
        for (int depth = 0; depth < 6; depth++) {
            float t;
            int ti = intersect(ro, rd, t);
            if (ti < 0) { radiance += through * skyRadiance(rd, primary); break; }
            int b = ti * 5;
            vec4 t0 = fetchT(u_tris, b);
            vec4 t1 = fetchT(u_tris, b + 1);
            vec4 t2 = fetchT(u_tris, b + 2);
            vec3 albedo = fetchT(u_tris, b + 3).xyz;
            vec3 emission = fetchT(u_tris, b + 4).xyz;
            float rough = t0.w, metal = t1.w, opacity = t2.w;
            vec3 hit = ro + rd * t;
            vec3 n = normalize(cross(t1.xyz, t2.xyz));
            if (dot(n, rd) > 0.0) n = -n;

            radiance += through * emission;

            if (opacity < 0.99) { // thin-pane glass
                float cosi = clamp(dot(-rd, n), 0.0, 1.0);
                float fres = 0.04 + 0.96 * pow(1.0 - cosi, 5.0);
                if (rnd() < fres) {
                    rd = normalize(reflect(rd, n));
                    ro = hit + rd * 1e-3;
                } else {
                    through *= (1.0 - opacity) * (0.5 + 0.5 * albedo) + opacity * albedo * 0.5;
                    ro = hit + rd * 1e-3;
                }
                primary = false;
                continue;
            }

            float ndl = dot(n, u_sun_dir);
            if (ndl > 0.0) {
                vec3 jd = normalize(u_sun_dir + cosineDir(u_sun_dir) * 0.012);
                vec3 tr = transmission(hit + n * 1e-3, jd);
                radiance += through * albedo * u_sun_col * ndl * tr;
            }

            if (rnd() < metal) {
                vec3 mirror = normalize(reflect(rd, n));
                vec3 lobe = cosineDir(mirror);
                rd = normalize(mix(mirror, lobe, rough * rough));
                if (dot(rd, n) <= 0.0) rd = mirror;
                through *= albedo;
            } else {
                rd = cosineDir(n);
                through *= albedo;
            }
            ro = hit + n * 1e-3;
            primary = false;

            float p = clamp(max(through.r, max(through.g, through.b)), 0.05, 1.0);
            if (depth >= 3) {
                if (rnd() > p) break;
                through /= p;
            }
        }
        return min(radiance, vec3(50.0));
    }

    void main() {
        uvec2 px = uvec2(gl_FragCoord.xy);
        g_state = (px.y * 9781u + px.x) * 6271u + u_pass * 26699u + 1u;
        rnd(); rnd(); // decorrelate
        float jx = rnd(), jy = rnd();
        float sx = ((gl_FragCoord.x - 0.5 + jx) / u_res.x * 2.0 - 1.0) * u_half * u_aspect;
        float sy = ((gl_FragCoord.y - 0.5 + jy) / u_res.y * 2.0 - 1.0) * u_half;
        vec3 rd = normalize(u_fwd + u_right * sx + u_up * sy);
        frag = vec4(trace(u_eye, rd), 1.0);
    }
"#;

/// The GPU render job — owns the GL objects; stepped from the UI thread each frame.
pub struct GpuTracer {
    program: glow::Program,
    vao: glow::VertexArray,
    fbo: glow::Framebuffer,
    accum: glow::Texture,
    tris_tex: glow::Texture,
    nodes_tex: glow::Texture,
    order_tex: glow::Texture,
    pub settings: Settings,
    pub scene_tris: usize,
    pub started: std::time::Instant,
    cam: Camera,
    sky: Sky,
    passes_done: u32,
    cancelled: bool,
    /// Cached last readback (so 💾 Save needs no GL access).
    pub last_rgba: Option<(usize, usize, Vec<u8>)>,
}

/// Upload a flat f32 texel stream as a TEXW-wide RGBA32F texture.
unsafe fn upload_stream(gl: &glow::Context, data: &[f32]) -> Result<glow::Texture, String> {
    let texels = (data.len() / 4).max(1);
    let h = texels.div_ceil(TEXW).max(1);
    let mut padded: Vec<f32>;
    let bytes: &[u8] = {
        let need = TEXW * h * 4;
        let src = if data.len() < need {
            padded = data.to_vec();
            padded.resize(need, 0.0);
            &padded[..]
        } else {
            data
        };
        std::slice::from_raw_parts(src.as_ptr() as *const u8, src.len() * 4)
    };
    let tex = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D, 0, glow::RGBA32F as i32, TEXW as i32, h as i32, 0,
        glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)),
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(tex)
}

impl GpuTracer {
    /// Compile the tracer, upload the scene, create the accumulation FBO.
    pub fn new(gl: &glow::Context, pack: &GpuPack, cam: Camera, sky: Sky, settings: Settings) -> Result<Self, String> {
        unsafe {
            // Program.
            let program = gl.create_program()?;
            let mut compiled = Vec::new();
            for (ty, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
                let sh = gl.create_shader(ty)?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    let log = gl.get_shader_info_log(sh);
                    gl.delete_shader(sh);
                    gl.delete_program(program);
                    return Err(format!("GPU tracer shader: {log}"));
                }
                gl.attach_shader(program, sh);
                compiled.push(sh);
            }
            gl.link_program(program);
            for sh in compiled {
                gl.detach_shader(program, sh);
                gl.delete_shader(sh);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("GPU tracer link: {log}"));
            }
            let vao = gl.create_vertex_array()?;

            // Scene streams.
            let tris_tex = upload_stream(gl, &pack.tris)?;
            let nodes_tex = upload_stream(gl, &pack.nodes)?;
            let order_tex = upload_stream(gl, &pack.order)?;

            // Accumulation FBO (RGBA32F).
            let accum = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(accum));
            gl.tex_image_2d(
                glow::TEXTURE_2D, 0, glow::RGBA32F as i32, settings.w as i32, settings.h as i32, 0,
                glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(accum), 0);
            let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if !ok {
                return Err("GPU tracer: RGBA32F framebuffer incomplete on this driver".into());
            }

            Ok(Self {
                program,
                vao,
                fbo,
                accum,
                tris_tex,
                nodes_tex,
                order_tex,
                settings,
                scene_tris: pack.tri_count,
                started: std::time::Instant::now(),
                cam,
                sky,
                passes_done: 0,
                cancelled: false,
                last_rgba: None,
            })
        }
    }

    pub fn passes_done(&self) -> u32 {
        self.passes_done
    }
    pub fn is_done(&self) -> bool {
        self.cancelled || self.passes_done >= self.settings.passes
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Trace up to `batch` passes (1 sample/pixel each) into the accumulation FBO. GL state is
    /// saved/restored around the draw so egui's own painter is unaffected.
    pub fn step(&mut self, gl: &glow::Context, batch: u32) {
        if self.is_done() {
            return;
        }
        let n = batch.min(self.settings.passes - self.passes_done);
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, self.settings.w as i32, self.settings.h as i32);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE); // additive accumulation
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));

            // Scene samplers on units 0..2.
            for (unit, (name, tex)) in [("u_tris", self.tris_tex), ("u_nodes", self.nodes_tex), ("u_order", self.order_tex)].iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*tex));
                if let Some(loc) = gl.get_uniform_location(self.program, name) {
                    gl.uniform_1_i32(Some(&loc), unit as i32);
                }
            }
            // Camera basis.
            let fwd = (self.cam.target - self.cam.eye).normalize();
            let right0 = fwd.cross(glam::Vec3::Z);
            let right = if right0.length_squared() < 0.5 { glam::Vec3::X } else { right0.normalize() };
            let up = right.cross(fwd);
            let set3 = |name: &str, v: [f32; 3]| {
                if let Some(loc) = gl.get_uniform_location(self.program, name) {
                    gl.uniform_3_f32(Some(&loc), v[0], v[1], v[2]);
                }
            };
            set3("u_eye", self.cam.eye.into());
            set3("u_fwd", fwd.into());
            set3("u_right", right.into());
            set3("u_up", up.into());
            set3("u_sun_dir", self.sky.sun_dir.into());
            set3("u_sun_col", self.sky.sun_col);
            set3("u_sky_col", self.sky.sky_col);
            set3("u_ground_col", self.sky.ground_col);
            if let Some(loc) = gl.get_uniform_location(self.program, "u_half") {
                gl.uniform_1_f32(Some(&loc), (self.cam.fov_deg.to_radians() * 0.5).tan());
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "u_aspect") {
                gl.uniform_1_f32(Some(&loc), self.settings.w as f32 / self.settings.h.max(1) as f32);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "u_res") {
                gl.uniform_2_f32(Some(&loc), self.settings.w as f32, self.settings.h as f32);
            }

            for _ in 0..n {
                if let Some(loc) = gl.get_uniform_location(self.program, "u_pass") {
                    gl.uniform_1_u32(Some(&loc), self.passes_done);
                }
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                self.passes_done += 1;
            }

            // Restore state egui's painter expects.
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::BLEND);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// Read the accumulation back, tone-map to RGBA8 (rows flipped — GL is bottom-up), and cache it.
    pub fn snapshot(&mut self, gl: &glow::Context) -> Option<(usize, usize, Vec<u8>)> {
        if self.passes_done == 0 {
            return self.last_rgba.clone();
        }
        let (w, h) = (self.settings.w, self.settings.h);
        let mut bytes = vec![0u8; w * h * 16]; // RGBA f32
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::FLOAT, glow::PixelPackData::Slice(Some(&mut bytes)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        let inv = 1.0 / self.passes_done as f32;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            let src_row = h - 1 - y; // flip vertically
            for x in 0..w {
                let s = (src_row * w + x) * 16;
                let d = (y * w + x) * 4;
                for c in 0..3 {
                    let f = f32::from_ne_bytes([bytes[s + c * 4], bytes[s + c * 4 + 1], bytes[s + c * 4 + 2], bytes[s + c * 4 + 3]]);
                    out[d + c] = tonemap8(f * inv);
                }
                out[d + 3] = 255;
            }
        }
        self.last_rgba = Some((w, h, out.clone()));
        Some((w, h, out))
    }

    /// Delete every GL object (call with the context before dropping).
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.accum);
            gl.delete_texture(self.tris_tex);
            gl.delete_texture(self.nodes_tex);
            gl.delete_texture(self.order_tex);
        }
    }
}
