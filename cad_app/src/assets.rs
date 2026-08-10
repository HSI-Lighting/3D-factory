//! Where the app's bundled data lives, at runtime.
//!
//! Everything used to be opened by a bare relative path — `assets/apertures/door.fbx` — which
//! resolves against the CURRENT WORKING DIRECTORY. That works while the app is launched from the
//! repo root and nowhere else. Installed under `C:\Program Files\…` and started from a Start Menu
//! shortcut, the working directory is whatever the shortcut says, and every one of those loads
//! fails: no doors, no windows, no CC0 texture library, and no message explaining why, because
//! each site treats a missing file as "nothing to offer".
//!
//! So resolve against the EXECUTABLE first. An installed layout puts `assets/` beside the binary;
//! a `cargo run` puts the binary three levels down in `target/…`, with `assets/` at the repo root.
//! Both are tried, in that order, and the working directory remains as a last resort so a dev
//! running from an odd place still gets what they expect.
//!
//! The one thing this must not do is guess. If nothing matches, it hands back the relative path
//! unchanged, so the error the caller reports names what it was actually looking for.

use std::path::{Path, PathBuf};

/// Resolve `rel` (e.g. `"assets/apertures/door.fbx"`) to a path that exists, or return it as-is.
pub fn path(rel: &str) -> PathBuf {
    for base in roots() {
        let p = base.join(rel);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(rel)
}

/// Directories `assets/` might sit under, most authoritative first.
///
/// Cached would be nicer, but these are called a handful of times at startup and on menu opens,
/// and a cache would have to be invalidated by nothing at all — not worth the static.
fn roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Installed: assets sit beside the binary.
            out.push(dir.to_path_buf());
            // `cargo run` / `target/release/simlux.exe`: the repo root is two levels up. Walk a
            // few, because a workspace target dir can be one deeper than expected.
            let mut up = dir;
            for _ in 0..4 {
                match up.parent() {
                    Some(p) => { out.push(p.to_path_buf()); up = p; }
                    None => break,
                }
            }
        }
    }
    // The working directory, which is what everything relied on before.
    out.push(PathBuf::from("."));
    out
}

/// True when `rel` resolves to something that exists — for menus that should hide an entry
/// rather than offer a button that silently does nothing.
pub fn exists(rel: &str) -> bool {
    path(rel).exists()
}

/// Convenience for callers that already hold a `Path`.
pub fn resolve(rel: &Path) -> PathBuf {
    path(&rel.to_string_lossy())
}

/// One startup line naming what was found, and where.
///
/// "Doors and windows are missing from the menus" is the shape this failure takes: every library
/// treats an absent folder as "nothing to offer", so a broken install looks like an app with no
/// content rather than an app that cannot find its files. One line on stderr turns that into a
/// five-second diagnosis, and it is the only way to confirm a PACKAGED build resolves its assets
/// without opening the GUI and clicking through three menus.
pub fn report() -> String {
    let count = |rel: &str| -> String {
        let p = path(rel);
        match std::fs::read_dir(&p) {
            Ok(rd) => format!("{}", rd.flatten().count()),
            Err(_) => "MISSING".to_string(),
        }
    };
    let root = path("assets");
    format!(
        "[assets] root={} apertures={} handles={} cc0/textures={} cc0/furniture={}",
        root.display(),
        count("assets/apertures"),
        count("assets/handles"),
        count("assets/cc0/textures"),
        count("assets/cc0/furniture"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repo's own assets resolve no matter where the test process is started from — which is
    /// the whole point, since a test binary runs from a different directory than the app.
    #[test]
    fn bundled_assets_resolve_from_the_executable() {
        let p = path("assets/apertures/window.obj");
        assert!(p.exists(), "the bundled window must be found, looked at {}", p.display());
        assert!(p.is_absolute() || p.starts_with("."),
            "a resolved asset should be rooted, got {}", p.display());
    }

    /// An unknown path comes back UNCHANGED rather than pointing somewhere plausible-but-wrong,
    /// so the caller's error message names the file it actually wanted.
    #[test]
    fn a_missing_asset_is_returned_as_written() {
        let p = path("assets/definitely/not/here.obj");
        assert_eq!(p, PathBuf::from("assets/definitely/not/here.obj"));
    }
}
