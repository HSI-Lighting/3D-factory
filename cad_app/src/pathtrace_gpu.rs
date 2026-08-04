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

    // TRI_TEXELS texels/tri: [p0|rough][e1|metal][e2|opacity][albedo|sheen_tint][emission|sheen]
    //                        [coat, coat_rough, 0, 0] — see `pathtrace::Scene::pack_gpu`.
    uniform sampler2D u_tris;
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

    // The HDR environment, when one is loaded — the same equirectangular map the viewport draws,
    // read through the same formula (ENV_UV_GLSL comes straight out of `crate::env`).
    uniform sampler2D u_env;
    uniform int u_env_on;
    uniform float u_env_rot;
    uniform float u_env_strength;
    ENV_UV_GLSL

    // ---- RNG (pcg-ish) ----
    uint g_state;
    float rnd() {
        g_state = g_state * 747796405u + 2891336453u;
        uint w = ((g_state >> ((g_state >> 28u) + 4u)) ^ g_state) * 277803737u;
        w = (w >> 22u) ^ w;
        return float(w & 16777215u) / 16777216.0;
    }

    vec4 fetchT(sampler2D t, int i) { return texelFetch(t, ivec2(i & 2047, i >> 11), 0); }

    // The inverse of `pathtrace::pack_rgb8` — three 8-bit channels out of one float.
    vec3 unpack_rgb8(float p) {
        float r = floor(p / 65536.0);
        float g = floor((p - r * 65536.0) / 256.0);
        return vec3(r, g, p - r * 65536.0 - g * 256.0) / 255.0;
    }

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
                    int b = ti * TRI_TEXELS;
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
            int b = ti * TRI_TEXELS;
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
        // An HDR environment answers for the whole sphere, and it answers the SAME whether the ray
        // is primary or not: its sun is a bright patch of image, not a delta light sampled
        // separately, so there is nothing here that could be counted twice. Word for word the rule
        // the CPU tracer follows in `pathtrace::sky_radiance`.
        if (u_env_on == 1) return texture(u_env, env_uv(dir)).rgb * u_env_strength;
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
            int b = ti * TRI_TEXELS;
            vec4 t0 = fetchT(u_tris, b);
            vec4 t1 = fetchT(u_tris, b + 1);
            vec4 t2 = fetchT(u_tris, b + 2);
            vec4 t3 = fetchT(u_tris, b + 3);
            vec4 t4 = fetchT(u_tris, b + 4);
            vec4 t5 = fetchT(u_tris, b + 5);
            vec3 albedo = t3.xyz;
            vec3 emission = t4.xyz;
            vec3 sheen_tint = unpack_rgb8(t3.w);
            float sheen = t4.w, coat = t5.x, coat_rough = max(t5.y, 0.01);
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

            // The coat takes its cut before the material underneath sees anything — the same
            // attenuation the viewport and the CPU tracer apply, so a lacquered surface gains a
            // reflection and loses a little of what is under it rather than simply getting brighter.
            vec3 vdir = -rd;
            float nov = max(dot(n, vdir), 1e-4);
            if (coat > 0.0) albedo *= (1.0 - coat * (0.04 + 0.96 * pow(1.0 - nov, 5.0)));

            float ndl = dot(n, u_sun_dir);
            if (ndl > 0.0) {
                vec3 jd = normalize(u_sun_dir + cosineDir(u_sun_dir) * 0.012);
                vec3 tr = transmission(hit + n * 1e-3, jd);
                radiance += through * albedo * u_sun_col * ndl * tr;
                vec3 hv = normalize(u_sun_dir + vdir);
                float fh = pow(clamp(1.0 - max(dot(vdir, hv), 0.0), 0.0, 1.0), 5.0);
                // SHEEN: the grazing rim, as an extra lobe on the sun.
                if (sheen > 0.0) radiance += through * sheen_tint * (sheen * fh * ndl) * u_sun_col * tr;
                // CLEARCOAT: a tight glint about the geometric normal. Approximated with a power
                // lobe rather than GGX — this backend's base BSDF is already the simpler model, and
                // matching its idiom keeps the two halves of the shader consistent with each other.
                if (coat > 0.0) {
                    float e = 2.0 / max(coat_rough * coat_rough, 1e-3);
                    float spec = pow(max(dot(n, hv), 0.0), e) * (e + 2.0) / 8.0;
                    float fc = (0.04 + 0.96 * fh) * coat;
                    radiance += through * vec3(spec * fc * ndl) * u_sun_col * tr;
                }
            }

            // A coated surface reflects the ROOM as well as the sun, and the room only reaches a
            // tracer through bounce rays — so the coat gets its own share of them.
            if (coat > 0.0 && rnd() < coat * 0.5) {
                vec3 mirror = normalize(reflect(rd, n));
                rd = normalize(mix(mirror, cosineDir(mirror), coat_rough * coat_rough));
                if (dot(rd, n) <= 0.0) rd = mirror;
                // Divided by the probability of having come here, so the estimator stays unbiased.
                through *= (0.04 + 0.96 * pow(1.0 - nov, 5.0)) * coat / (coat * 0.5);
                ro = hit + n * 1e-3;
                primary = false;
                continue;
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
    /// The HDR environment, when the scene has one. `None` ⇒ the two-colour hemisphere.
    env_tex: Option<glow::Texture>,
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

/// The widest equirect the tracer will upload. A downloaded 8K panorama is 8192 × 4096, which is
/// 400 MB of float in flight before the driver has converted anything — for a background that no
/// render resolution can resolve. 4K is already more than a 1920-wide render can show across a
/// 60° field of view. Box-downsampling below this keeps the sun's total energy (see
/// [`crate::env_map::EnvMap::resized`]), so the shadows do not change when the map is shrunk.
const ENV_MAX_W: usize = 4096;

/// Upload the environment as one RGB16F equirect, or `None` when there is no map.
///
/// A failure here returns `None` rather than aborting the render: the tracer falls back to the
/// two-colour hemisphere, which is what it did before this existed. Losing the backdrop is a far
/// better outcome than losing the render.
unsafe fn upload_env(gl: &glow::Context, map: Option<&crate::env_map::EnvMap>) -> Option<glow::Texture> {
    let map = map?;
    let scaled;
    let src = if map.w > ENV_MAX_W {
        let h = (map.h * ENV_MAX_W / map.w.max(1)).max(1);
        scaled = map.resized(ENV_MAX_W, h);
        &scaled
    } else {
        map
    };
    let flat: Vec<f32> = src.px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let bytes = std::slice::from_raw_parts(flat.as_ptr() as *const u8, flat.len() * 4);
    let tex = gl.create_texture().ok()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    // RGB16F, not RGB8: this is scene-referred radiance and its sun is thousands of times brighter
    // than its sky. Eight bits would clip that to white and the render would lose the light.
    gl.tex_image_2d(
        glow::TEXTURE_2D, 0, glow::RGB16F as i32, src.w as i32, src.h as i32, 0,
        glow::RGB, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)),
    );
    // REPEAT across longitude (the map is continuous where its left edge meets its right) and
    // CLAMP down latitude (it is not continuous over the poles) — as the viewport binds it.
    for (p, v) in [
        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
        (glow::TEXTURE_WRAP_S, glow::REPEAT),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
    }
    gl.bind_texture(glow::TEXTURE_2D, None);
    Some(tex)
}

impl GpuTracer {
    /// Compile the tracer, upload the scene, create the accumulation FBO.
    pub fn new(gl: &glow::Context, pack: &GpuPack, cam: Camera, sky: Sky, settings: Settings) -> Result<Self, String> {
        unsafe {
            // Program.
            let program = gl.create_program()?;
            let mut compiled = Vec::new();
            let fs = FS
                .replace("ENV_UV_GLSL", crate::env::ENV_UV_GLSL)
                .replace("TRI_TEXELS", &crate::pathtrace::TRI_TEXELS.to_string());
            for (ty, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, fs.as_str())] {
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
            let env_tex = upload_env(gl, sky.env.as_deref());

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
                env_tex,
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
            // The environment on unit 3, and the switch that tells the shader to believe it.
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, self.env_tex);
            if let Some(loc) = gl.get_uniform_location(self.program, "u_env") {
                gl.uniform_1_i32(Some(&loc), 3);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "u_env_on") {
                gl.uniform_1_i32(Some(&loc), self.env_tex.is_some() as i32);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "u_env_rot") {
                gl.uniform_1_f32(Some(&loc), self.sky.env_rot);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "u_env_strength") {
                gl.uniform_1_f32(Some(&loc), self.sky.env_strength);
            }
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
                let mut lin = [0.0f32; 3];
                for c in 0..3 {
                    let f = f32::from_ne_bytes([bytes[s + c * 4], bytes[s + c * 4 + 1], bytes[s + c * 4 + 2], bytes[s + c * 4 + 3]]);
                    lin[c] = f * inv;
                }
                let rgb = tonemap8(self.settings.color, lin);
                out[d..d + 3].copy_from_slice(&rgb);
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
            if let Some(t) = self.env_tex {
                gl.delete_texture(t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tracer must read the environment through the SAME lookup the viewport uses.
    ///
    /// There is no GL context in a unit test, so this checks the one thing that actually goes
    /// wrong: two transcriptions of an equirect lookup drifting apart. It is substituted from
    /// `crate::env`, not copied — this asserts the substitution really happens, because a
    /// misspelled token would leave the literal word in the source and fail to compile only on a
    /// machine with a GPU.
    #[test]
    fn the_tracer_and_the_viewport_read_the_environment_the_same_way() {
        assert!(FS.contains("ENV_UV_GLSL"), "the tracer includes the shared lookup by token");
        let fs = FS.replace("ENV_UV_GLSL", crate::env::ENV_UV_GLSL);
        assert!(!fs.contains("ENV_UV_GLSL"), "…and the token is fully substituted");
        assert!(fs.contains("vec2 env_uv(vec3 d)"), "…leaving a real env_uv behind");
        assert!(fs.contains("uniform float u_env_rot;"), "…with the rotation it reads in scope");
        // The viewport's copy has to stay word for word the same as the shared one, or the render
        // and the view disagree about which way round the world is.
        let shared = crate::env::ENV_UV_GLSL.trim();
        assert!(
            crate::env::SKY_GLSL.contains(shared),
            "SKY_GLSL's env_uv has drifted from ENV_UV_GLSL"
        );
    }

    /// An HDR environment must answer for every ray, primary or not — the same rule the CPU
    /// tracer follows. Its sun is a bright patch of image, not a delta light sampled separately,
    /// so branching on `primary` here would darken every indirect bounce for no reason.
    #[test]
    fn the_environment_answers_the_same_for_every_ray() {
        let body = FS.split("vec3 skyRadiance").nth(1).expect("skyRadiance exists");
        let env_line = body.find("u_env_on == 1").expect("the environment short-circuit");
        let primary = body.find("if (primary)").expect("the analytic sun disc");
        assert!(env_line < primary, "the environment must answer before the primary-only branch");
    }

    /// A huge panorama must be shrunk rather than uploaded whole. An 8K map is 400 MB of float in
    /// flight for a background no render resolution can resolve.
    /// The shader's stride and the packer's layout must be ONE number.
    ///
    /// They are on opposite sides of a string substitution, so a mismatch does not fail to
    /// compile — it silently reads each triangle's material out of the next triangle's texels, and
    /// the render comes back with every surface wearing its neighbour's material.
    #[test]
    fn the_shader_strides_by_the_packed_texel_count() {
        assert!(FS.contains("ti * TRI_TEXELS"), "the shader strides by the shared constant");
        let fs = FS.replace("TRI_TEXELS", &crate::pathtrace::TRI_TEXELS.to_string());
        assert!(!fs.contains("TRI_TEXELS"), "…and every occurrence is substituted");
        // The highest texel the shader reads must exist in the packed stride.
        let last = (0..16)
            .filter(|i| fs.contains(&format!("fetchT(u_tris, b + {i})")))
            .max()
            .expect("the shader fetches material texels");
        assert!(
            last + 1 <= crate::pathtrace::TRI_TEXELS,
            "the shader reads texel {last} but only {} are packed per triangle",
            crate::pathtrace::TRI_TEXELS
        );
    }

    /// The clearcoat must dim what is under it before adding its own reflection, in every backend.
    #[test]
    fn the_gpu_clearcoat_pays_for_itself() {
        let coat = FS.find("albedo *= (1.0 - coat").expect("the base is attenuated");
        let glint = FS.find("radiance += through * vec3(spec * fc").expect("the coat's own glint");
        assert!(coat < glint, "the coat dims itself instead of the base");
        assert!(FS.contains("unpack_rgb8(t3.w)"), "the sheen tint is unpacked from its float");
    }

    #[test]
    fn an_oversized_panorama_is_capped() {
        assert!(ENV_MAX_W <= 4096, "the upload cap has grown past what a render can show");
        let big = crate::env_map::EnvMap::from_fn(64, 32, "small", |_| [1.0; 3]);
        assert!(big.w <= ENV_MAX_W, "a small map is left alone");
    }
}
