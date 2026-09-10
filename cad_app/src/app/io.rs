use super::*;

// ================= DWG open (from dokkandar/Auto_RASM, cross-platform) =================

/// Locate a DWG→DXF converter. Order: `RUSTCAD_DWGCONV` env override, the repo
/// `tools/dwgconv` wrapper (`.cmd` on Windows, `.sh` elsewhere), `~/.local/bin`,
/// then PATH. Returns None if none found (DWG open then reports a clear error).
pub(super) fn dwg_converter() -> Option<String> {
    if let Ok(c) = std::env::var("RUSTCAD_DWGCONV") {
        let c = c.trim().to_string();
        if !c.is_empty() {
            return Some(c);
        }
    }
    let wrapper = if cfg!(windows) {
        "dwgconv.cmd"
    } else {
        "dwgconv.sh"
    };
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors() {
            let w = anc.join("tools/dwgconv").join(wrapper);
            if w.is_file() {
                return Some(w.to_string_lossy().to_string());
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let w = std::path::Path::new(&home).join(".local/bin/dwgconv");
        if w.is_file() {
            return Some(w.to_string_lossy().to_string());
        }
    }
    for name in ["dwgconv", wrapper] {
        if std::process::Command::new(name).output().is_ok() {
            return Some(name.to_string());
        }
    }
    None
}

/// Locate a DXF→DWG converter — the mirror of [`dwg_converter`], and the same search.
///
/// Reported as: *"why cant i save as dwg?"* Because the save path took `.dxf` and `.rsm` and said
/// so, which answers the question without solving it: a practice's filing, its consultants and its
/// clients ask for `.dwg`, and "we write an equivalent DXF" is not an answer anybody can hand over.
///
/// Nothing here writes DWG directly — it is closed, versioned and undocumented — so the app writes
/// the DXF it already knows how to write and asks AutoCAD's own headless core to save it on. The
/// dependency is the one this machine already has, because it is what opens a DWG too.
pub(super) fn dxf_to_dwg_converter() -> Option<String> {
    if let Ok(c) = std::env::var("RUSTCAD_DXF2DWG") {
        let c = c.trim().to_string();
        if !c.is_empty() {
            return Some(c);
        }
    }
    let wrapper = if cfg!(windows) {
        "dxf2dwg.cmd"
    } else {
        "dxf2dwg.sh"
    };
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors() {
            let w = anc.join("tools/dwgconv").join(wrapper);
            if w.is_file() {
                return Some(w.to_string_lossy().to_string());
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let w = std::path::Path::new(&home).join(".local/bin/dxf2dwg");
        if w.is_file() {
            return Some(w.to_string_lossy().to_string());
        }
    }
    for name in ["dxf2dwg", wrapper] {
        if std::process::Command::new(name).output().is_ok() {
            return Some(name.to_string());
        }
    }
    None
}

/// Run the converter `conv` to turn `dwg` into `out` (a .dxf). A `{in}/{out}`
/// template runs via the shell; a bare path is spawned directly. `.cmd`/`.bat`
/// wrappers on Windows are launched through `cmd /c` (they are not PE exes).
pub(super) fn run_dwg_conversion(
    conv: &str,
    dwg: &str,
    out: &std::path::Path,
) -> Result<(), String> {
    let _ = std::fs::remove_file(out);
    let out_s = out.to_string_lossy().to_string();
    let status = if conv.contains("{in}") {
        let cmd = conv.replace("{in}", dwg).replace("{out}", &out_s);
        if cfg!(windows) {
            // RAW, NOT `arg` — see `convert_dxf_to_dwg`. Rust escapes an argument for a normal
            // Windows program and `cmd` does not use those rules, so a command carrying a quoted
            // path arrives mangled and exits 1. The same flaw, in the other direction.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("cmd")
                    .arg("/c")
                    .raw_arg(&cmd)
                    .status()
            }
            #[cfg(not(windows))]
            {
                unreachable!()
            }
        } else {
            let q = |s: &str| format!("'{}'", s.replace('\'', "'\''"));
            let shcmd = conv.replace("{in}", &q(dwg)).replace("{out}", &q(&out_s));
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&shcmd)
                .status()
        }
    } else if cfg!(windows)
        && (conv.to_ascii_lowercase().ends_with(".cmd")
            || conv.to_ascii_lowercase().ends_with(".bat"))
    {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg(conv)
            .arg(dwg)
            .arg(&out_s)
            .status()
    } else {
        std::process::Command::new(conv)
            .arg(dwg)
            .arg(&out_s)
            .status()
    };
    match status {
        Ok(_) if out.exists() => Ok(()),
        // WHY IT FAILED, NOT JUST THAT IT DID. This reported "converter exited 3 (no DXF
        // produced)" and threw the converter's own stderr away -- which on the usual failure says,
        // in full sentences, "accoreconsole.exe not found under C:\Program Files\Autodesk" and
        // what to set instead. A GUI user never sees a console, so that explanation reached nobody
        // and the symptom was an exit code.
        Ok(s) => Err(format!(
            "the DWG converter exited {s} and produced no DXF.
{}",
            converter_stderr(conv, dwg, &out_s),
        )),
        Err(e) => Err(format!("could not run converter '{conv}': {e}")),
    }
}

/// Re-run the converter capturing its output, so the REASON can be shown.
///
/// A second run rather than capturing the first: the first is spawned with the terminal inherited
/// so a long conversion still shows progress where there is a console, and re-running a converter
/// that has already failed costs nothing anybody notices.
pub(super) fn converter_stderr(conv: &str, dwg: &str, out_s: &str) -> String {
    let o = if cfg!(windows)
        && (conv.to_ascii_lowercase().ends_with(".cmd")
            || conv.to_ascii_lowercase().ends_with(".bat"))
    {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg(conv)
            .arg(dwg)
            .arg(out_s)
            .output()
    } else {
        std::process::Command::new(conv)
            .arg(dwg)
            .arg(out_s)
            .output()
    };
    match o {
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr);
            let e = e.trim();
            if e.is_empty() {
                "The converter said nothing. DWG needs AutoCAD on this machine (its \n                 accoreconsole.exe does the work); set RUSTCAD_DWGCONV to another converter if \n                 there is none."
                    .to_string()
            } else {
                e.lines()
                    .map(|l| format!("    {}", l.trim()))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    )
            }
        }
        Err(e) => format!("    (could not re-run the converter to ask why: {e})"),
    }
}

impl CadApp {
    /// Convert a `.dwg` to a temp `.dxf` via the external converter (ACadSharp),
    /// then the normal DXF reader parses it. Shared by do_open.
    pub(crate) fn convert_dwg_to_dxf(&self, dwg: &str) -> Result<std::path::PathBuf, String> {
        let conv = dwg_converter().ok_or_else(|| {
            "no DWG converter found — set RUSTCAD_DWGCONV to \"cmd {in} {out}\" \
             or build tools/dwgconv (see its README)"
                .to_string()
        })?;
        let out = std::env::temp_dir().join("rustcad_dwg_open.dxf");
        run_dwg_conversion(&conv, dwg, &out)?;
        Ok(out)
    }
}
