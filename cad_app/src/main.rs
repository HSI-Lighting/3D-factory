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
mod illuminaire; // Illuminaire — a library of fittings: a 2D block + a photometric file
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
    // A CRASH MUST LEAVE SOMETHING BEHIND. A Windows GUI build has no console, so a panic prints
    // its message to a stderr nobody sees and the process simply vanishes — which is exactly what
    // "the app crashed while calculating" looks like from the outside, and it is not diagnosable.
    //
    // Written beside the executable, so it travels with the install rather than into a profile
    // directory the user would have to be told how to find. Appended, because the second crash is
    // usually the informative one and overwriting loses the first.
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let where_ = info.location().map_or("?".to_string(), |l| format!("{}:{}", l.file(), l.line()));
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let text = format!(
            "\n=== SIMLUX PANIC ===\nbuild {} ({})\nat {where_}\n{msg}\n\n{bt}\n",
            option_env!("SIMLUX_BUILD_NO").unwrap_or("?"),
            option_env!("SIMLUX_BUILD").unwrap_or("unknown"),
        );
        eprintln!("{text}");
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("simlux-crash.log"))
            {
                let _ = f.write_all(text.as_bytes());
            }
        }
    }));

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
    // VERIFY THE HOOK IN THE SHIPPED BINARY. A crash reporter that has never been seen to fire is
    // a guess; this makes it one command to prove, on the exact build a user is running:
    //
    //     SIMLUX_TEST_PANIC=1 simlux.exe      -> writes simlux-crash.log beside the exe
    //
    // Behind an env var rather than a menu item, because it is for whoever is diagnosing a crash,
    // not for the person who just lost their work.
    if std::env::var("SIMLUX_TEST_PANIC").is_ok() {
        panic!("deliberate test panic — SIMLUX_TEST_PANIC was set");
    }


    // BEFORE ANYTHING CAN RUN A BOOLEAN — the generators, an autoloaded fixture, a reopened
    // project. csgrs keeps its tolerance in a `OnceLock` whose first READER initialises it to the
    // default, so this has to be the earliest thing that touches the crate or it silently no-ops.
    // At the default, cuts on work smaller than about 300 mm come back with the wrong volume and
    // no error of any kind. See `cad_solid::BOOLEAN_TOLERANCE` for the measurements.
    match cad_solid::init_boolean_tolerance() {
        Ok(t)  => eprintln!("[simlux] boolean tolerance {t:e}"),
        Err(e) => eprintln!("[simlux] WARNING: {e}"),
    }

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
            let mut a = app::CadApp::default();
            // THE LIBRARY IS THE APP'S, so it is read once here rather than in `default()` —
            // which the test suite calls hundreds of times and must not have the developer's
            // own fittings leaking into.
            a.load_illuminaire_library();
            Ok(Box::new(a))
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
