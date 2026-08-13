mod aci_picker;
mod app;
mod color; // colour management — sRGB decode/encode + the display view transform (AgX &c.)
mod command;
mod dbg_recorder;
mod dock;
mod door_mat; // what the parametric door is made of — one palette, preview + build read it
mod env_map; // HDR image-based lighting — a real environment instead of the analytic sky
mod env; // environment lighting — the analytic sky, its SH ambient, and the AO settings
mod factory;   // 3D Factory — cad_solid wired into the app
mod gpu;
mod assets; // where bundled data lives at runtime — see the module docs
mod handles; // swappable door-handle library (assets/handles/handles.json)
mod hatch_trace;
mod light;
mod light_report; // the SIMLUX calculation written out as a standalone HTML report
mod light3d;
mod material_graph; // Materials Factory — node-based material authoring (compiles to renderer params)
mod mesh_io;      // OBJ furniture import
mod mesh_preview; // CPU preview of a parametric build, shown before it is inserted
mod param_editor;
mod matball;   // CPU material-ball preview — the same BRDF and sky the viewport uses
mod pathtrace; // in-app progressive path tracer — shared core + CPU backend
mod proc_tex;  // Rust twin of the shader's procedural evaluation (path tracer + preview read it)
mod pathtrace_gpu; // GPU backend: the same tracer in a GL 3.3 fragment shader
mod radiance_export; // offline Radiance render export (.rad geometry + gensky sky)
#[cfg(test)]
mod render_probe; // headless villa render → PNG, so a change to the LOOK can be judged by looking
#[cfg(test)]
mod report_figs; // renders the Phase 2–4 report's figures from the code they document
mod settings;
mod simlux_io;
mod solar;      // Radiance-based sun position for daylight rendering
mod texture_set; // PBR texture-set folders: filename → map slot, and the loader that follows it
mod theme;
mod varreg;
// wall feature logic now lives in the `cad_wall` crate (see ARCHITECTURE.md).

fn main() -> Result<(), eframe::Error> {
    // Say which build this is and what data it found, before anything else can go wrong. Both
    // questions have cost real time here: a repair was twice run against a stale binary, and a
    // bundled library that fails to resolve shows up as EMPTY MENUS rather than as an error, so
    // a broken install looks like an app with no content. Two lines on stderr, once.
    eprintln!(
        "[simlux] build {} ({})",
        option_env!("SIMLUX_BUILD_NO").unwrap_or("?"),
        option_env!("SIMLUX_BUILD").unwrap_or("unknown"),
    );
    eprintln!("{}", assets::report());

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("SIMLUX — Lighting Designer"),
        ..Default::default()
    };
    eframe::run_native(
        "simlux",
        options,
        Box::new(|cc| {
            // Load Geist + JetBrains Mono before the first frame (THEME_SYSTEM §5.7).
            theme::install_fonts(&cc.egui_ctx);
            // Follow the desktop's text-scaling setting. winit applies the monitor
            // scale for us, but NOT GNOME's `text-scaling-factor` (Settings ▸
            // Accessibility ▸ Large Text / the fractional text-scale slider), so
            // the UI would otherwise render tiny on a system scaled >1.0. Apply it
            // once as egui's zoom factor (it multiplies onto the native
            // pixels-per-point); the user can still Ctrl+± / Ctrl+scroll to adjust.
            let zoom = desktop_text_scale();
            if (zoom - 1.0).abs() > f32::EPSILON {
                cc.egui_ctx.set_zoom_factor(zoom);
            }
            Ok(Box::new(app::CadApp::default()))
        }),
    )
}

/// Read the desktop's global text-scaling factor so SIMLUX's UI matches the
/// system font size. On GNOME this is `org.gnome.desktop.interface
/// text-scaling-factor`. Returns 1.0 when unavailable (non-GNOME, non-Linux, or
/// any error), and is clamped to a sane [0.5, 4.0] range.
#[cfg(target_os = "linux")]
fn desktop_text_scale() -> f32 {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                if let Ok(f) = s.trim().parse::<f32>() {
                    if f.is_finite() {
                        return f.clamp(0.5, 4.0);
                    }
                }
            }
        }
    }
    1.0
}

/// Non-Linux platforms: winit already applies the native OS DPI scale, so no
/// extra text-scaling lookup is needed.
#[cfg(not(target_os = "linux"))]
fn desktop_text_scale() -> f32 {
    1.0
}
