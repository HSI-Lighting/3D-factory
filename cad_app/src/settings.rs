// User-Environment Settings — RUST_CAD's analogue of AutoCAD SYSVARS.
//
// Cryptic short names (`SpTGSZ`, `GrpEnb`, …) follow a project-wide
// convention: no underscores, mixed case, 5–7 characters. See
// `feedback_rust_cad_settings_naming.md` in the memory store.
//
// Per-field doc comments are the source of truth for what each setting
// does. The settings window in `app.rs` mirrors them as plain-English
// labels next to the cryptic name.
//
// Persisted to `$HOME/.config/rust_cad/user_env.txt` — a one-line-per-
// setting format (`KEY = VALUE`). Survives app restarts; falls back to
// `Default::default()` if the file is missing or unparseable.

#![allow(non_snake_case)]

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct UserEnv {
    // ---- snap & picking sizes (pixels / percent) ----
    /// Object-snap target height in pixels. Cursor must be within this many
    /// screen pixels of a candidate snap point (or entity, depending on the
    /// kind) for the snap to fire.
    pub SpTGSZ: u8,
    /// Pickbox height in pixels — the square inside which a click is
    /// considered "on" a dobject (selection / hit-test tolerance).
    /// TENTATIVE: `HitTolPx` (see Variables.md §13) may supersede this.
    /// Keep for now; revisit when selection hit-testing is centralized.
    pub PkBxSz: u8,
    /// Crosshair size as percentage of the viewport's shorter side.
    pub CrsHrS: u8,

    // ---- dialogs ----
    /// Attribute entry dialog on INSERT. true = popup dialog, false = prompt.
    /// TENTATIVE: no attribute system exists yet. Keep for forward-compat;
    /// revisit when (and if) blocks + attributes land.
    pub AtDlgM: bool,
    /// Attribute prompting during INSERT.
    /// TENTATIVE: same as `AtDlgM` — no attribute system yet.
    pub AtPrmM: bool,
    /// Dialog boxes for PLOT, etc. true = show, false = command-line only.
    pub CmDlgM: bool,
    /// Suppress file-navigation dialogs (true = use OS dialogs; false = type).
    pub FlDlgM: bool,

    // ---- editing behaviour ----
    /// Edge-mode for TRIM / EXTEND. ON = treat cutting / boundary edges as
    /// their infinite extensions for "imaginary intersection" cuts. OFF =
    /// use only intersections on the visible curve. Default ON (matches the
    /// AutoCAD `EDGEMODE 1` convention most CAD users expect today).
    pub EdgMod: bool,
    /// Default fillet radius (AutoCAD `FILLETRAD`). Set inline by passing
    /// `fillet <r>` — the new value persists across sessions.
    pub FltRad: f64,
    /// Default chamfer distance along the FIRST picked line
    /// (AutoCAD `CHAMFERA`).
    pub ChmDs1: f64,
    /// Default chamfer distance along the SECOND picked line
    /// (AutoCAD `CHAMFERB`).
    pub ChmDs2: f64,
    /// Default offset distance (AutoCAD `OFFSETDIST`). Set inline by
    /// passing `offset <d>` — the new value persists across sessions
    /// and bare `offset` re-uses it. Initial default 1.0.
    pub OfsDis: f64,
    /// Default wall thickness for the `wall` drafting command. Two
    /// parallel lines are drawn ±`WlThk/2` from an implicit centerline
    /// between the user's two clicks. Set inline with `wall <t>`;
    /// persists across sessions. Initial default 0.20 (200mm).
    pub WlThk: f64,
    /// Default text height for the `text` drafting command, in world
    /// units. Persists across sessions (mirrors WlThk/OfsDis). Initial
    /// 0.25 — a sensible default for typical drawings.
    pub TxHt:  f64,
    /// Wall Centerline visible. Renders the implicit centerline of
    /// every `Geom::Wall` as a dashed half-alpha overlay on top of
    /// the two solid side lines. Useful while developing the wall-
    /// aware fillet/extend/trim semantics; flip off for production.
    /// Default `true` for now.
    pub WlCnL: bool,
    /// Trim mode shared by Fillet and Chamfer (AutoCAD `TRIMMODE`).
    /// `true` (default) → trim originals back to the new corner.
    /// `false` → keep originals full-length, add the arc/bevel as a
    /// separate dobject ("No Trim" mode). Toggle via `t` or `nt` at
    /// the fillet/chamfer prompt. Persistent across the session +
    /// across runs (saved to `user_env.txt`).
    pub TrmMd: bool,

    // ---- display ----
    /// Dragging display during MOVE/COPY. 0=off, 1=on, 2=auto.
    pub DrDspM: u8,
    /// Display the classic menu bar at the top of the window.
    pub MnuBar: bool,
    /// Show toolbar/ribbon tooltips on hover.
    pub TltEnb: bool,
    /// Tooltips on dobject rollover (hover over a dobject in the canvas).
    pub RllTp:  bool,
    /// Preview-highlight a dobject when the cursor is over it (before click).
    pub SelPrv: bool,
    /// Highlight selected dobjects with a distinct color.
    pub HltSel: bool,
    /// Frame display of wipeouts. 0=off, 1=on, 2=on for selection only.
    pub WpFrmM: u8,

    // ---- grips ----
    /// Enable / disable grip handles on selected dobjects.
    pub GrpEnb: bool,
    /// Show grips on dobjects that live inside blocks (when blocks land).
    pub GrpBlk: bool,
    /// Unselected grip colour (RGB packed: 0xRRGGBB).
    pub GrClrU: u32,
    /// Selected (hot) grip colour (RGB packed).
    pub GrClrS: u32,
    /// Grip size in pixels.
    pub GrpSz:  u8,
    /// Grip HOVER + GRAB radius in screen pixels. When a dobject is
    /// selected and the cursor comes within this many pixels of one
    /// of its grip points, that grip highlights (preview = "this is
    /// what your click will grab"); the same threshold is the click
    /// tolerance for entering grip-drag. 25 px is roomy enough that
    /// the user doesn't have to pixel-aim. AutoCAD has no exact
    /// equivalent; LibreCAD's grip "hot radius" defaults to ~20 px.
    pub GrpHvR: u8,

    // ---- grid + CARD ----
    /// Background grid display (AutoCAD GRIDMODE). ON = render the grid
    /// overlay; OFF = clean canvas. F7 toggles.
    pub GrdEnb: bool,
    /// Snap cursor to grid intersections (AutoCAD SNAPMODE). When ON and
    /// the cursor is in drafting mode, world coords are rounded to the
    /// nearest `GrdSpc` multiple before being used for click capture
    /// or live preview. F9 toggles. Osnap (object snap) wins over this.
    pub GrdSnp: bool,
    /// Grid spacing in world units (AutoCAD GRIDUNIT). Same value used
    /// for the display grid AND for snap-to-grid rounding.
    pub GrdSpc: f64,
    /// **CARD** — cardinal-directions drafting lock. When ON and a
    /// "from" point exists (line's first endpoint, move base, copy
    /// base, …), the cursor's world position is projected onto whichever
    /// of the two axes from that anchor is closer, so the result is ONLY
    /// horizontal or vertical. Toggles: F8, the CARD status badge, or
    /// the `card` command (`card on` / `card off`). Settings files
    /// written before the rename used the key `OrtEnb`; the loader still
    /// accepts it (the one permitted occurrence of the legacy name).
    pub CrdEnb: bool,

    // ---- UCS indicator (origin marker) ----
    /// User Coordinate System indicator on/off (AutoCAD `UCSICON`).
    /// When true, the canvas renders a small origin marker (red dot
    /// + X / Y axis arrows). See `UcsMod` for placement behaviour.
    pub UcsIcn: bool,
    /// UCS icon placement mode:
    ///   0 = bottom-left corner ALWAYS (default, simplest legend)
    ///   1 = anchor at world (0,0) when visible, else fall back to
    ///       the corner (AutoCAD's `UCSICON ORigin` behaviour)
    /// Change via the settings panel — most users want the corner
    /// pin, but precision-drafting workflows benefit from seeing the
    /// origin live in the drawing.
    pub UcsMod: u8,
    /// Path to a PNG/SVG used as the user's avatar on the X-axis of
    /// the UCS icon. Empty string → fall back to the "User logo"
    /// placeholder rectangle. Persisted across sessions so the user
    /// only sets it once.
    pub UcsAvP: String,

    // ---- selection ----
    /// Selection-drag activation hold time, in milliseconds. In
    /// select-mode, a press becomes a window-drag ONLY after the user
    /// has held the primary button this long. A fast press-drag-
    /// release without holding past this threshold is treated as a
    /// click on the release position (the user fumbled a click rather
    /// than asking for a window). Range 50–2000 ms; AutoCAD-like
    /// default is 250 ms. The rubber-band preview honors the same
    /// gate, so nothing visual happens until the threshold passes.
    pub SelDmTm: u16,

    // ---- APX (approximate / draft display) ----
    /// Dot-anchor strategy when rendering in APX (user-toggled draft
    /// mode). 0 = bbox center (default, simple/uniform). 1 = primitive
    /// center (Circle.center, Line midpoint, Polyline centroid — TBD
    /// per type). 2 = first vertex of the dobject. The APX mode
    /// itself is toggled by a button in the status bar — no auto-
    /// trigger; user-controlled.
    pub LodAnc: u8,

    // ---- external references ----
    /// External-reference demand-loading mode.
    /// 0 = off, 1 = on, 2 = on with copy (work on a temp duplicate).
    pub XrLdMd: u8,
    /// Path for temporary xref copies (empty → system temp dir).
    pub XrTmpP: String,
}

impl Default for UserEnv {
    fn default() -> Self {
        Self {
            SpTGSZ: 16,
            PkBxSz: 10,
            CrsHrS: 5,
            AtDlgM: true,
            AtPrmM: true,
            CmDlgM: true,
            FlDlgM: true,
            EdgMod: true,
            FltRad: 0.0,
            ChmDs1: 0.0,
            ChmDs2: 0.0,
            OfsDis: 1.0,
            WlThk:  0.20,
            TxHt:   0.25,
            WlCnL:  true,
            TrmMd: true,
            DrDspM: 2,
            MnuBar: false,
            TltEnb: true,
            RllTp:  true,
            SelPrv: true,
            HltSel: true,
            WpFrmM: 2,
            GrpEnb: true,
            GrpBlk: false,
            GrClrU: 0x4099FF,    // light blue
            GrClrS: 0xFF6464,    // red-pink
            GrpSz:  4,
            GrpHvR: 25,
            GrdEnb: true,
            GrdSnp: false,
            GrdSpc: 10.0,
            CrdEnb: false,
            UcsIcn: true,
            UcsMod: 0,                  // corner by default
            UcsAvP: String::new(),
            SelDmTm: 250,
            LodAnc: 0,
            XrLdMd: 2,
            XrTmpP: String::new(),
        }
    }
}

impl UserEnv {
    fn config_path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config/rust_cad/user_env.txt"))
    }

    /// Load from disk, or fall back to `Default::default()` if the file
    /// is missing, unreadable, or malformed.
    pub fn load() -> Self {
        let mut env = Self::default();
        let Some(path) = Self::config_path() else { return env; };
        let Ok(text) = fs::read_to_string(&path) else { return env; };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else { continue; };
            env.set(k.trim(), v.trim());
        }
        env
    }

    /// Write to disk. Logs failures by returning Err; the caller decides
    /// whether to surface them to the user.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::config_path() else {
            return Err(std::io::Error::other("no HOME"));
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut s = String::new();
        s.push_str("# RUST_CAD User-Environment Settings\n");
        s.push_str("# Cryptic short names — see source / settings window for the plain-English description.\n\n");
        // Order matches the struct so the file reads top-down by section.
        let push_u8   = |s: &mut String, k: &str, v: u8| s.push_str(&format!("{} = {}\n", k, v));
        let push_u32  = |s: &mut String, k: &str, v: u32| s.push_str(&format!("{} = 0x{:06X}\n", k, v));
        let push_bool = |s: &mut String, k: &str, v: bool| s.push_str(&format!("{} = {}\n", k, v));
        let push_str  = |s: &mut String, k: &str, v: &str| s.push_str(&format!("{} = {}\n", k, v));
        push_u8(&mut s, "SpTGSZ", self.SpTGSZ);
        push_u8(&mut s, "PkBxSz", self.PkBxSz);
        push_u8(&mut s, "CrsHrS", self.CrsHrS);
        push_bool(&mut s, "AtDlgM", self.AtDlgM);
        push_bool(&mut s, "AtPrmM", self.AtPrmM);
        push_bool(&mut s, "CmDlgM", self.CmDlgM);
        push_bool(&mut s, "FlDlgM", self.FlDlgM);
        push_bool(&mut s, "EdgMod", self.EdgMod);
        let push_f64 = |s: &mut String, k: &str, v: f64| s.push_str(&format!("{} = {}\n", k, v));
        push_f64(&mut s, "FltRad", self.FltRad);
        push_f64(&mut s, "ChmDs1", self.ChmDs1);
        push_f64(&mut s, "ChmDs2", self.ChmDs2);
        push_f64(&mut s, "OfsDis", self.OfsDis);
        push_f64(&mut s, "WlThk",  self.WlThk);
        push_f64(&mut s, "TxHt",   self.TxHt);
        push_bool(&mut s, "WlCnL", self.WlCnL);
        push_bool(&mut s, "TrmMd", self.TrmMd);
        push_u8(&mut s, "DrDspM", self.DrDspM);
        push_bool(&mut s, "MnuBar", self.MnuBar);
        push_bool(&mut s, "TltEnb", self.TltEnb);
        push_bool(&mut s, "RllTp",  self.RllTp);
        push_bool(&mut s, "SelPrv", self.SelPrv);
        push_bool(&mut s, "HltSel", self.HltSel);
        push_u8(&mut s, "WpFrmM", self.WpFrmM);
        push_bool(&mut s, "GrpEnb", self.GrpEnb);
        push_bool(&mut s, "GrpBlk", self.GrpBlk);
        push_u32(&mut s, "GrClrU", self.GrClrU);
        push_u32(&mut s, "GrClrS", self.GrClrS);
        push_u8(&mut s, "GrpSz",  self.GrpSz);
        push_u8(&mut s, "GrpHvR", self.GrpHvR);
        push_bool(&mut s, "GrdEnb", self.GrdEnb);
        push_bool(&mut s, "GrdSnp", self.GrdSnp);
        push_f64(&mut s, "GrdSpc", self.GrdSpc);
        push_bool(&mut s, "CrdEnb", self.CrdEnb);
        let push_u16_dec = |s: &mut String, k: &str, v: u16| s.push_str(&format!("{} = {}\n", k, v));
        push_bool(&mut s, "UcsIcn", self.UcsIcn);
        push_u8(&mut s, "UcsMod", self.UcsMod);
        push_str(&mut s, "UcsAvP", &self.UcsAvP);
        push_u16_dec(&mut s, "SelDmTm", self.SelDmTm);
        push_u8(&mut s, "LodAnc", self.LodAnc);
        push_u8(&mut s, "XrLdMd", self.XrLdMd);
        push_str(&mut s, "XrTmpP", &self.XrTmpP);
        fs::write(&path, s)
    }

    /// Assign by cryptic key. Unknown keys are ignored (forward-compatible
    /// with files written by a newer version). Malformed values are dropped
    /// silently — the field keeps its prior value.
    fn set(&mut self, key: &str, val: &str) {
        let parse_bool = |s: &str| -> Option<bool> {
            match s.to_ascii_lowercase().as_str() {
                "true" | "1" | "on" | "yes"  => Some(true),
                "false"| "0" | "off"| "no"   => Some(false),
                _ => None,
            }
        };
        let parse_u32 = |s: &str| -> Option<u32> {
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                s.parse().ok()
            }
        };
        match key {
            "SpTGSZ" => if let Ok(v) = val.parse() { self.SpTGSZ = v; }
            "PkBxSz" => if let Ok(v) = val.parse() { self.PkBxSz = v; }
            "CrsHrS" => if let Ok(v) = val.parse() { self.CrsHrS = v; }
            "AtDlgM" => if let Some(v) = parse_bool(val) { self.AtDlgM = v; }
            "AtPrmM" => if let Some(v) = parse_bool(val) { self.AtPrmM = v; }
            "CmDlgM" => if let Some(v) = parse_bool(val) { self.CmDlgM = v; }
            "FlDlgM" => if let Some(v) = parse_bool(val) { self.FlDlgM = v; }
            "EdgMod" => if let Some(v) = parse_bool(val) { self.EdgMod = v; }
            "FltRad" => if let Ok(v) = val.parse() { self.FltRad = v; }
            "ChmDs1" => if let Ok(v) = val.parse() { self.ChmDs1 = v; }
            "ChmDs2" => if let Ok(v) = val.parse() { self.ChmDs2 = v; }
            "OfsDis" => if let Ok(v) = val.parse() { self.OfsDis = v; }
            "WlThk"  => if let Ok(v) = val.parse() { self.WlThk  = v; }
            "TxHt"   => if let Ok(v) = val.parse() { self.TxHt   = v; }
            "WlCnL"  => if let Some(v) = parse_bool(val) { self.WlCnL = v; }
            "TrmMd"  => if let Some(v) = parse_bool(val) { self.TrmMd = v; }
            "DrDspM" => if let Ok(v) = val.parse() { self.DrDspM = v; }
            "MnuBar" => if let Some(v) = parse_bool(val) { self.MnuBar = v; }
            "TltEnb" => if let Some(v) = parse_bool(val) { self.TltEnb = v; }
            "RllTp"  => if let Some(v) = parse_bool(val) { self.RllTp  = v; }
            "SelPrv" => if let Some(v) = parse_bool(val) { self.SelPrv = v; }
            "HltSel" => if let Some(v) = parse_bool(val) { self.HltSel = v; }
            "WpFrmM" => if let Ok(v) = val.parse() { self.WpFrmM = v; }
            "GrpEnb" => if let Some(v) = parse_bool(val) { self.GrpEnb = v; }
            "GrpBlk" => if let Some(v) = parse_bool(val) { self.GrpBlk = v; }
            "GrClrU" => if let Some(v) = parse_u32(val) { self.GrClrU = v; }
            "GrClrS" => if let Some(v) = parse_u32(val) { self.GrClrS = v; }
            "GrpSz"  => if let Ok(v) = val.parse() { self.GrpSz = v; }
            "GrpHvR" => if let Ok(v) = val.parse() { self.GrpHvR = v; }
            "GrdEnb" => if let Some(v) = parse_bool(val) { self.GrdEnb = v; }
            "GrdSnp" => if let Some(v) = parse_bool(val) { self.GrdSnp = v; }
            "GrdSpc" => if let Ok(v) = val.parse() { self.GrdSpc = v; }
            // "OrtEnb" = legacy key from before the CARD rename — still
            // accepted so old user_env.txt files keep their setting.
            "CrdEnb" | "OrtEnb" => if let Some(v) = parse_bool(val) { self.CrdEnb = v; }
            "UcsIcn" => if let Some(v) = parse_bool(val) { self.UcsIcn = v; }
            "UcsMod" => if let Ok(v) = val.parse() { self.UcsMod = v; }
            "UcsAvP" => self.UcsAvP = val.to_string(),
            "SelDmTm" => if let Ok(v) = val.parse() { self.SelDmTm = v; }
            "LodAnc" => if let Ok(v) = val.parse() { self.LodAnc = v; }
            "XrLdMd" => if let Ok(v) = val.parse() { self.XrLdMd = v; }
            "XrTmpP" => self.XrTmpP = val.to_string(),
            _ => {}     // unknown — forward-compatible
        }
    }
}
