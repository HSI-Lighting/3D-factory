//! cad_app — the SIMLUX application library.
//!
//! Everything except the binary's `main()` lives here, so the same code serves
//! the desktop app (`simlux` bin → `cad_app` lib) and the test suite.
//!
//! Split out of `main.rs` so the 87k-line `app` module family can be worked on
//! by three developers without the binary entry point (panic hook, window
//! title, `eframe::run_native`) owning every `mod` declaration.

mod aci_picker;
pub mod app;
pub mod assets; // where bundled data lives at runtime — see the module docs
mod calc; // command-line calculator + lazy user variables (pure, no UI deps)
mod color; // colour management — sRGB decode/encode + the display view transform (AgX &c.)
mod command;
mod dbg_recorder;
mod dock;
mod door_mat; // what the parametric door is made of — one palette, preview + build read it
mod env; // environment lighting — the analytic sky, its SH ambient, and the AO settings
mod env_map; // HDR image-based lighting — a real environment instead of the analytic sky
mod factory; // 3D Factory — cad_solid wired into the app
mod gpu;
mod handles; // swappable door-handle library (assets/handles/handles.json)
mod hatch_trace;
mod illuminaire; // Illuminaire — a library of fittings: a 2D block + a photometric file
mod isolux; // isolux lines — marching squares over a calculated field
mod layer_glyphs;
mod light;
mod light3d;
mod light_report; // the SIMLUX calculation written out as a standalone HTML report
mod light_store; // the last calculation, kept beside the drawing so closing the app does not lose it
mod matball; // CPU material-ball preview — the same BRDF and sky the viewport uses
mod material_graph; // Materials Factory — node-based material authoring (compiles to renderer params)
mod mesh_io; // OBJ furniture import
mod mesh_preview; // CPU preview of a parametric build, shown before it is inserted
mod param_editor;
mod pathtrace; // in-app progressive path tracer — shared core + CPU backend
mod pathtrace_gpu; // GPU backend: the same tracer in a GL 3.3 fragment shader
mod proc_tex; // Rust twin of the shader's procedural evaluation (path tracer + preview read it)
mod radiance_export; // offline Radiance render export (.rad geometry + gensky sky)
#[cfg(test)]
mod render_probe; // headless villa render → PNG, so a change to the LOOK can be judged by looking
mod report; // report generation — page layout, PDF output, and the options that drive them
#[cfg(test)]
mod report_figs; // renders the Phase 2–4 report's figures from the code they document
mod settings;
mod simlux_io;
mod solar; // Radiance-based sun position for daylight rendering
mod texture_set; // PBR texture-set folders: filename → map slot, and the loader that follows it
pub mod theme;
mod varreg; // 2D glyph helpers shared by the canvas rails (dim/zoom rail icons)
            // wall feature logic now lives in the `cad_wall` crate (see ARCHITECTURE.md).

pub use app::CadApp;

/// THE BUILD, IN THE TITLE BAR.
///
/// It was already on the first line of stderr, in every session dump and at the top of every
/// report — and not one of those is in front of somebody who has just double-clicked a shortcut.
/// Four fixed bugs were reported as still broken, twice over, because the desktop app was three
/// commits behind and nothing on screen said so. A stale build looks exactly like a fix that did
/// not work; the difference is one glance, if the number is somewhere a person actually looks.
///
/// Packaging now installs as it builds, so this should never disagree with the latest cut. It is
/// here for the day something goes wrong with that — an old shortcut, a second copy on another
/// drive, an install that half-finished — because that is precisely when nobody thinks to check.
///
/// `dev` rather than `?` for the commit when there is none: a binary built straight from `cargo
/// run` is not a build anybody cut, and saying so is more useful than a question mark.
pub fn window_title() -> String {
    format!(
        "SIMLUX — Lighting Designer   ·   build {} ({})",
        option_env!("SIMLUX_BUILD_NO").unwrap_or("?"),
        option_env!("SIMLUX_BUILD").unwrap_or("dev"),
    )
}

/// THE WINDOW SAYS WHICH BUILD IT IS.
#[cfg(test)]
mod the_window_names_its_build {
    /// The stamp is PRESENT and is the real one — not a placeholder, and not a hard-coded string
    /// that would go on saying "build 30" for ever.
    ///
    /// `build.rs` reads `packaging/build-number.txt` and the git hash, so on any checkout with a
    /// repo these are real values; the test asserts against the same environment the binary was
    /// compiled with rather than against a literal.
    #[test]
    fn the_title_carries_the_build_number_and_the_commit() {
        let t = super::window_title();
        assert!(t.starts_with("SIMLUX"), "the app lost its name: {t:?}");
        let n = option_env!("SIMLUX_BUILD_NO").unwrap_or("?");
        let c = option_env!("SIMLUX_BUILD").unwrap_or("dev");
        assert!(
            t.contains(n),
            "the title does not name the build number ({n}): {t:?}"
        );
        assert!(
            t.contains(c),
            "the title does not name the commit ({c}): {t:?}"
        );
    }

    /// AND `build.rs` ACTUALLY STAMPED IT. A title that faithfully prints "?" and "dev" would pass
    /// the test above while telling a user nothing — which is the state this whole thing exists to
    /// make impossible. Skipped where there is no git checkout to read, since that is a legitimate
    /// way to build and must not fail.
    #[test]
    fn the_stamp_is_a_real_one_in_this_repo() {
        if !std::path::Path::new("../.git").exists() {
            return;
        }
        assert_ne!(
            option_env!("SIMLUX_BUILD"),
            Some("unknown"),
            "build.rs did not resolve the commit in a checkout that has one",
        );
        let n = option_env!("SIMLUX_BUILD_NO").unwrap_or("?");
        assert!(
            n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty(),
            "the build number came through as {n:?} rather than a number",
        );
    }
}
