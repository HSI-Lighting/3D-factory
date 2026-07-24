mod aci_picker;
mod app;
mod command;
mod dbg_recorder;
mod dock;
mod factory;   // 3D Factory — cad_solid wired into the app
mod gpu;
mod hatch_trace;
mod light;
mod light3d;
mod mesh_io;      // OBJ furniture import
mod param_editor;
mod settings;
mod simlux_io;
mod theme;
mod varreg;
// wall feature logic now lives in the `cad_wall` crate (see ARCHITECTURE.md).

fn main() -> Result<(), eframe::Error> {
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
