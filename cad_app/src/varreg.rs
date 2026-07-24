//! varreg.rs — the canonical variable registry. Single source of truth for
//! the settings page and the command line. Each row is presentation +
//! metadata; the WIRED ones map to real UserEnv fields via env_get/env_set.
//!
//! After editing this file, SETTINGS.md's tables are regenerated automatically
//! by the pre-commit hook (.githooks/pre-commit → tools/gen-settings.ps1).
//! To regenerate by hand: powershell -ExecutionPolicy Bypass -File tools/gen-settings.ps1
#![allow(non_snake_case)]
use crate::settings::UserEnv;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status { Active, Planned, Stub, Tentative }

#[derive(Clone, Copy)]
pub enum Kind {
    Bool,
    U8 { min: u8, max: u8 },
    Choice(&'static [&'static str]),   // u8 index into the names
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Color,                             // 0xRRGGBB packed u32
    Text,
}

pub struct Var {
    pub name: &'static str,
    pub section: &'static str,
    pub desc: &'static str,
    pub kind: Kind,
    pub status: Status,
    pub default: &'static str,   // canonical default, as a string parsed per Kind
    pub wired: bool,             // true => maps to a real UserEnv field (editable+persisted)
}

pub static VARS: &[Var] = &[
    // ───────────────────────────────────────────────────────────────────
    // Display & Visual Feedback
    // ───────────────────────────────────────────────────────────────────
    Var { name: "AperBx",  section: "Display & Visual Feedback", desc: "Show the snap-target box around the crosshair (size = SpTGSZ)", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "BkgPlt",  section: "Display & Visual Feedback", desc: "Print/plot in the background while you keep working", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "CrsACol", section: "Display & Visual Feedback", desc: "Crossing-selection area colour", kind: Kind::Color, status: Status::Planned, default: "0x78E678", wired: false },
    Var { name: "CrsHrS",  section: "Display & Visual Feedback", desc: "Crosshair size (screen %)", kind: Kind::U8 { min: 1, max: 100 }, status: Status::Planned, default: "5", wired: true },
    Var { name: "DrDspM",  section: "Display & Visual Feedback", desc: "Dragging display during MOVE/COPY", kind: Kind::Choice(&["off", "on", "auto"]), status: Status::Planned, default: "2", wired: true },
    Var { name: "GalVw",   section: "Display & Visual Feedback", desc: "Block gallery view on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "HltSel",  section: "Display & Visual Feedback", desc: "Highlight selected objects", kind: Kind::Bool, status: Status::Planned, default: "true", wired: true },
    Var { name: "HpQckP",  section: "Display & Visual Feedback", desc: "Hatch quick preview on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "ImgHlt",  section: "Display & Visual Feedback", desc: "Image frame highlight on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "IntsCol", section: "Display & Visual Feedback", desc: "Colour of the intersection (∩) markers where objects cross", kind: Kind::Color, status: Status::Planned, default: "0xFF5A5A", wired: false },
    Var { name: "IntsDsp", section: "Display & Visual Feedback", desc: "Show the intersection (∩) markers where objects cross", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "LnFade",  section: "Display & Visual Feedback", desc: "Line fading in edit mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "LtGlyD",  section: "Display & Visual Feedback", desc: "Light glyph display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "LyLkFd",  section: "Display & Visual Feedback", desc: "Locked-layer fade percentage", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Planned, default: "50", wired: false },
    Var { name: "MTxtFx",  section: "Display & Visual Feedback", desc: "Mtext fixed-width editor on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "OleHid",  section: "Display & Visual Feedback", desc: "Hide OLE objects on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PcBnd",   section: "Display & Visual Feedback", desc: "Point-cloud bounding-box display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PcClpF",  section: "Display & Visual Feedback", desc: "Show the crop-boundary outline of a 3D point cloud (laser scan)", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PrvFlt",  section: "Display & Visual Feedback", desc: "Object types skipped during hover preview-highlight", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RllTp",   section: "Display & Visual Feedback", desc: "Tooltips on dobject rollover", kind: Kind::Bool, status: Status::Planned, default: "true", wired: true },
    Var { name: "RvClCrM", section: "Display & Visual Feedback", desc: "How revision clouds are drawn (the change-marking cloud outline)", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RvClGrp", section: "Display & Visual Feedback", desc: "Revcloud grip display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "SelAr",   section: "Display & Visual Feedback", desc: "Selection area effect", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "SelPrv",  section: "Display & Visual Feedback", desc: "Preview highlight of selection", kind: Kind::Bool, status: Status::Planned, default: "true", wired: true },
    Var { name: "SelPrvL", section: "Display & Visual Feedback", desc: "Selection preview dobject limit", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "2000", wired: false },
    Var { name: "TrkPth",  section: "Display & Visual Feedback", desc: "Tracking path display mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TrnDsp",  section: "Display & Visual Feedback", desc: "Draw objects' transparency (see-through) level on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TryIco",  section: "Display & Visual Feedback", desc: "Tray icon display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TryTim",  section: "Display & Visual Feedback", desc: "Tray notification timeout", kind: Kind::Int { min: 1, max: 1_000_000 }, status: Status::Stub, default: "5", wired: false },
    Var { name: "WinACol", section: "Display & Visual Feedback", desc: "Window-selection area colour", kind: Kind::Color, status: Status::Planned, default: "0x78AAFF", wired: false },
    Var { name: "WmfBkg",  section: "Display & Visual Feedback", desc: "WMF background colour", kind: Kind::Color, status: Status::Stub, default: "0xFFFFFF", wired: false },
    Var { name: "WmfFrg",  section: "Display & Visual Feedback", desc: "WMF foreground colour", kind: Kind::Color, status: Status::Stub, default: "0x000000", wired: false },
    Var { name: "WpFrmM",  section: "Display & Visual Feedback", desc: "Frame display of wipeouts", kind: Kind::Choice(&["off", "on", "on for selection only"]), status: Status::Stub, default: "2", wired: true },
    Var { name: "LodAnc",  section: "Display & Visual Feedback", desc: "APX draft dot-anchor strategy", kind: Kind::Choice(&["bbox center", "primitive center", "first vertex"]), status: Status::Planned, default: "0", wired: true },

    // ───────────────────────────────────────────────────────────────────
    // Selection & Grips
    // ───────────────────────────────────────────────────────────────────
    Var { name: "GrClrS",  section: "Selection & Grips", desc: "Selected (hot) grip colour", kind: Kind::Color, status: Status::Planned, default: "0xFF6464", wired: true },
    Var { name: "GrClrU",  section: "Selection & Grips", desc: "Unselected grip colour", kind: Kind::Color, status: Status::Planned, default: "0x4099FF", wired: true },
    Var { name: "GrpBlk",  section: "Selection & Grips", desc: "Show grips inside blocks", kind: Kind::Bool, status: Status::Stub, default: "false", wired: true },
    Var { name: "GrpEnb",  section: "Selection & Grips", desc: "Enable/disable grips", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
    Var { name: "GrpObjL", section: "Selection & Grips", desc: "Maximum dobjects for grip display", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "1000", wired: false },
    Var { name: "GrpSz",   section: "Selection & Grips", desc: "Grip size (pixels)", kind: Kind::U8 { min: 1, max: 20 }, status: Status::Planned, default: "4", wired: true },
    Var { name: "GrpTip",  section: "Selection & Grips", desc: "Grip hover tooltips on/off", kind: Kind::Bool, status: Status::Stub, default: "true", wired: false },
    Var { name: "HidTxt",  section: "Selection & Grips", desc: "Hide text during move/rotate", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "ObjIsoM", section: "Selection & Grips", desc: "Object isolation mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "OsnNdLg", section: "Selection & Grips", desc: "Osnap node legacy mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "OsnOpt",  section: "Selection & Grips", desc: "Object snap options", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "63", wired: false },
    Var { name: "PkAdd",   section: "Selection & Grips", desc: "Selection add mode", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "PkAuto",  section: "Selection & Grips", desc: "Implied window selection", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "PkDrag",  section: "Selection & Grips", desc: "Selection by dragging", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "PkFrst",  section: "Selection & Grips", desc: "Noun/verb selection", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "SelCyc",  section: "Selection & Grips", desc: "Selection cycling on/off", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "SelOfSc", section: "Selection & Grips", desc: "Select off-screen dobjects", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "SubSelM", section: "Selection & Grips", desc: "Subobject selection mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    // wired additions kept in Selection & Grips per spec
    Var { name: "SelDmTm", section: "Selection & Grips", desc: "Selection-drag activation hold time (ms)", kind: Kind::Int { min: 50, max: 2000 }, status: Status::Active, default: "250", wired: true },
    Var { name: "GrpHvR",  section: "Selection & Grips", desc: "Grip hover/grab radius (pixels)", kind: Kind::U8 { min: 4, max: 80 }, status: Status::Active, default: "25", wired: true },

    // ───────────────────────────────────────────────────────────────────
    // Object Snaps & Precision
    // ───────────────────────────────────────────────────────────────────
    Var { name: "OsnCrd",  section: "Object Snaps & Precision", desc: "Osnap coordinate override keyboard", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "PkBxSz",  section: "Object Snaps & Precision", desc: "Pickbox height (pixels)", kind: Kind::U8 { min: 1, max: 40 }, status: Status::Tentative, default: "10", wired: true },
    Var { name: "PolAdA",  section: "Object Snaps & Precision", desc: "Polar additional angles", kind: Kind::Text, status: Status::Planned, default: "", wired: false },
    Var { name: "PolAng",  section: "Object Snaps & Precision", desc: "Polar angle setting", kind: Kind::Int { min: 0, max: 360 }, status: Status::Planned, default: "90", wired: false },
    Var { name: "PolDst",  section: "Object Snaps & Precision", desc: "Polar snap distance", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "1", wired: false },
    Var { name: "PolMod",  section: "Object Snaps & Precision", desc: "Polar tracking mode", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "SpTGSZ",  section: "Object Snaps & Precision", desc: "Object-snap target height (pixels)", kind: Kind::U8 { min: 4, max: 80 }, status: Status::Active, default: "16", wired: true },
    Var { name: "TmpOvr",  section: "Object Snaps & Precision", desc: "Temporary override keys", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Editing & Behavior
    // ───────────────────────────────────────────────────────────────────
    Var { name: "AtDlgM",  section: "Editing & Behavior", desc: "Attribute entry dialog on INSERT", kind: Kind::Bool, status: Status::Tentative, default: "true", wired: true },
    Var { name: "AtPrmM",  section: "Editing & Behavior", desc: "Attribute prompting during INSERT", kind: Kind::Bool, status: Status::Tentative, default: "true", wired: true },
    Var { name: "BActBM",  section: "Editing & Behavior", desc: "Block action bar display mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "BlkEdLk", section: "Editing & Behavior", desc: "Lock block editor from editing", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "BlkEdtr", section: "Editing & Behavior", desc: "Block editor open/close state", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "BlkMrL",  section: "Editing & Behavior", desc: "Block MRU list length", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "5", wired: false },
    Var { name: "BndTyp",  section: "Editing & Behavior", desc: "Xref bind type", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "CmDlgM",  section: "Editing & Behavior", desc: "Dialog boxes for PLOT, etc.", kind: Kind::Bool, status: Status::Stub, default: "true", wired: true },
    Var { name: "DblClkE", section: "Editing & Behavior", desc: "Double-click editing on/off", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "EdgMod",  section: "Editing & Behavior", desc: "Edge-mode for trim/extend", kind: Kind::Bool, status: Status::Planned, default: "true", wired: true },
    Var { name: "HpMaxA",  section: "Editing & Behavior", desc: "Maximum hatch area for preview", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "100", wired: false },
    Var { name: "HpObjW",  section: "Editing & Behavior", desc: "Hatch dobject warning limit", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "500", wired: false },
    Var { name: "HpSep",   section: "Editing & Behavior", desc: "Separate hatch dobjects on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "InpHMd",  section: "Editing & Behavior", desc: "Dynamic input history display mode", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "MTjigS",  section: "Editing & Behavior", desc: "Mtext sample string for jig", kind: Kind::Text, status: Status::Stub, default: "Sample", wired: false },
    Var { name: "PedAcc",  section: "Editing & Behavior", desc: "Suppress PEDIT convert prompt", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PrsPul",  section: "Editing & Behavior", desc: "Presspull behavior mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RefPtTp", section: "Editing & Behavior", desc: "Reference path type", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "SavFid",  section: "Editing & Behavior", desc: "Save visual fidelity for annotative", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "SbyLyr",  section: "Editing & Behavior", desc: "SetByLayer mode", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "SrfAsc",  section: "Editing & Behavior", desc: "Surface associativity", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TblInd",  section: "Editing & Behavior", desc: "Table cell indicator on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TblTbr",  section: "Editing & Behavior", desc: "Table toolbar on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "XEdit",   section: "Editing & Behavior", desc: "Edit in-place on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "XFdCtl",  section: "Editing & Behavior", desc: "Ref-edit object fading", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // File & Save
    // ───────────────────────────────────────────────────────────────────
    Var { name: "AudCtl",  section: "File & Save", desc: "Create audit report file", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "AutoPub", section: "File & Save", desc: "Automatic publish on save/close", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "DgnMpP",  section: "File & Save", desc: "DGN mapping file path", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "DwgChk",  section: "File & Save", desc: "Check for non-Autodesk DWG files", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "IsvBak",  section: "File & Save", desc: "Incremental save backup creation", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "IsvPrc",  section: "File & Save", desc: "Incremental save percentage", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "LogFlM",  section: "File & Save", desc: "Log file on/off", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "LogFlP",  section: "File & Save", desc: "Log file path", kind: Kind::Text, status: Status::Planned, default: "", wired: false },
    Var { name: "OpnPrt",  section: "File & Save", desc: "Open partial DWG file", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RcovMd",  section: "File & Save", desc: "Drawing recovery mode", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "SavFP",   section: "File & Save", desc: "Automatic save file path", kind: Kind::Text, status: Status::Planned, default: "./autosave/", wired: false },
    Var { name: "SavTim",  section: "File & Save", desc: "Automatic save interval (minutes)", kind: Kind::Int { min: 1, max: 1_000_000 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "SigWarn", section: "File & Save", desc: "Digital signature warning", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "SldChk",  section: "File & Save", desc: "3D solid validation on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TrstPth", section: "File & Save", desc: "Trusted file paths", kind: Kind::Text, status: Status::Stub, default: "", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Xrefs & Images
    // ───────────────────────────────────────────────────────────────────
    Var { name: "XrLdMd",  section: "Xrefs & Images", desc: "External-reference demand-loading", kind: Kind::Choice(&["off", "on", "on with copy"]), status: Status::Active, default: "2", wired: true },
    Var { name: "XrTmpP",  section: "Xrefs & Images", desc: "Path for temporary xref copies", kind: Kind::Text, status: Status::Active, default: "", wired: true },
    Var { name: "XrCtl",   section: "Xrefs & Images", desc: "Xref log file on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "XrLyr",   section: "Xrefs & Images", desc: "Default layer for xref insertion", kind: Kind::Text, status: Status::Stub, default: "0", wired: false },
    Var { name: "XrNtfy",  section: "Xrefs & Images", desc: "Xref change notification", kind: Kind::Bool, status: Status::Stub, default: "true", wired: false },
    Var { name: "XrTyp",   section: "Xrefs & Images", desc: "Default xref type", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "XdwFd",   section: "Xrefs & Images", desc: "Xref drawing fade percentage", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "RastDpi", section: "Xrefs & Images", desc: "Raster image DPI for plotting", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "300", wired: false },
    Var { name: "RastPrc", section: "Xrefs & Images", desc: "Raster image memory percentage", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Stub, default: "20", wired: false },
    Var { name: "RastThr", section: "Xrefs & Images", desc: "Raster image memory threshold", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "100", wired: false },
    Var { name: "OleQlty", section: "Xrefs & Images", desc: "OLE plot quality", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "1", wired: false },
    Var { name: "OleStrt", section: "Xrefs & Images", desc: "OLE application startup on load", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PdfShx",  section: "Xrefs & Images", desc: "PDF SHX text handling", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PdfShxL", section: "Xrefs & Images", desc: "PDF SHX text layer", kind: Kind::Text, status: Status::Stub, default: "", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // UI & Workspace
    // ───────────────────────────────────────────────────────────────────
    Var { name: "DobMenu", section: "UI & Workspace", desc: "Enterprise CUI menu file", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "LokUI",   section: "UI & Workspace", desc: "Lock toolbars/palettes position", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "MnuBar",  section: "UI & Workspace", desc: "Display the classic menu bar", kind: Kind::Bool, status: Status::Planned, default: "false", wired: true },
    Var { name: "MnuCtl",  section: "UI & Workspace", desc: "Menu control (screen menu)", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "NavBar",  section: "UI & Workspace", desc: "Navigation bar display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "NavCube", section: "UI & Workspace", desc: "ViewCube display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PalOpq",  section: "UI & Workspace", desc: "Palette transparency", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Stub, default: "100", wired: false },
    Var { name: "QpLoc",   section: "UI & Workspace", desc: "Quick-properties location", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "QpMod",   section: "UI & Workspace", desc: "Quick-properties mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RibSta",  section: "UI & Workspace", desc: "Ribbon minimized state", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "ScrnBx",  section: "UI & Workspace", desc: "Screen menu boxes (legacy)", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "ShctMn",  section: "UI & Workspace", desc: "Shortcut menu on/off", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "StartUp", section: "UI & Workspace", desc: "Startup dialog mode", kind: Kind::Choice(&["off", "on"]), status: Status::Planned, default: "0", wired: false },
    Var { name: "TbCust",  section: "UI & Workspace", desc: "Toolbar customize on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TltEnb",  section: "UI & Workspace", desc: "Show toolbar/ribbon tooltips", kind: Kind::Bool, status: Status::Planned, default: "true", wired: true },
    Var { name: "TltMrg",  section: "UI & Workspace", desc: "Tooltip merge on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TltTrn",  section: "UI & Workspace", desc: "Tooltip transparency", kind: Kind::U8 { min: 0, max: 100 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "TpPalP",  section: "UI & Workspace", desc: "Tool palette path", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "TxtEd",   section: "UI & Workspace", desc: "Text editor application", kind: Kind::Text, status: Status::Stub, default: "", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Plot & Publish
    // ───────────────────────────────────────────────────────────────────
    Var { name: "PapUpd",  section: "Plot & Publish", desc: "Paper-size update warning", kind: Kind::Bool, status: Status::Stub, default: "true", wired: false },
    Var { name: "PStPlc",  section: "Plot & Publish", desc: "Plot style policy for new drawings", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "0", wired: false },
    Var { name: "PubAll",  section: "Plot & Publish", desc: "Publish all sheets", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PubHch",  section: "Plot & Publish", desc: "Publish hatch on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // System & Performance
    // ───────────────────────────────────────────────────────────────────
    Var { name: "FlDlgM",  section: "System & Performance", desc: "Suppress file-navigation dialogs", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
    Var { name: "FntAlt",  section: "System & Performance", desc: "Alternate font when font not found", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "FntMap",  section: "System & Performance", desc: "Font mapping file path", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "LspAsD",  section: "System & Performance", desc: "Load acad.lsp into every drawing", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "MxActVp", section: "System & Performance", desc: "Maximum active viewports", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "64", wired: false },
    Var { name: "MxSort",  section: "System & Performance", desc: "Maximum list sort size", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "1000", wired: false },
    Var { name: "PrxNot",  section: "System & Performance", desc: "Proxy dobject notice", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PrxShw",  section: "System & Performance", desc: "Proxy dobject display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "PrxWeb",  section: "System & Performance", desc: "Proxy web search on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "StdViol", section: "System & Performance", desc: "Standards-violation notification", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "SysMon",  section: "System & Performance", desc: "System-variable monitor on/off", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "TreMax",  section: "System & Performance", desc: "Tree memory limit", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "100000", wired: false },
    Var { name: "TxtFil",  section: "System & Performance", desc: "Text fill on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "TxtQlt",  section: "System & Performance", desc: "Text quality", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "50", wired: false },
    Var { name: "UntMod",  section: "System & Performance", desc: "Unit display mode", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "0", wired: false },
    Var { name: "WhipArc", section: "System & Performance", desc: "Arc/circle smoothness", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "8", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // View & Navigation
    // ───────────────────────────────────────────────────────────────────
    Var { name: "GeoLoc",  section: "View & Navigation", desc: "Geolocation marker visibility", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "LayTab",  section: "View & Navigation", desc: "Model/Layout tab display", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "RtDsp",   section: "View & Navigation", desc: "Real-time pan/zoom display", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "StepSz",  section: "View & Navigation", desc: "Walk/fly step size", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Stub, default: "1", wired: false },
    Var { name: "StpPrSc", section: "View & Navigation", desc: "Walk/fly steps per second", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "30", wired: false },
    Var { name: "SunPrW",  section: "View & Navigation", desc: "Sun properties window on/off", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "UcsOrt",  section: "View & Navigation", desc: "Orthographic UCS toggle", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "VtDur",   section: "View & Navigation", desc: "Smooth view transition duration", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "300", wired: false },
    Var { name: "VtEnbl",  section: "View & Navigation", desc: "Smooth view transition on/off", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "VtFps",   section: "View & Navigation", desc: "Smooth view transition speed (FPS)", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "60", wired: false },
    Var { name: "VwUpdA",  section: "View & Navigation", desc: "View update automatic", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "ZmFact",  section: "View & Navigation", desc: "Mouse wheel zoom factor", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "0.0015", wired: false },
    Var { name: "ZmWhl",   section: "View & Navigation", desc: "Mouse wheel zoom direction", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Miscellaneous
    // ───────────────────────────────────────────────────────────────────
    Var { name: "Chrma",   section: "Miscellaneous", desc: "Colour-book display mode", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "LyrDlgM", section: "Miscellaneous", desc: "Layer properties manager mode", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "LyrFlA",  section: "Miscellaneous", desc: "Layer-filter alert on/off", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "LyrNtf",  section: "Miscellaneous", desc: "Layer notification on/off", kind: Kind::Bool, status: Status::Planned, default: "false", wired: false },
    Var { name: "MTxtEd",  section: "Miscellaneous", desc: "Multiline text editor application", kind: Kind::Text, status: Status::Stub, default: "", wired: false },
    Var { name: "PrjNam",  section: "Miscellaneous", desc: "Project file search path", kind: Kind::Text, status: Status::Planned, default: "", wired: false },
    Var { name: "SsmAuto", section: "Miscellaneous", desc: "Sheet Set Manager auto open", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },
    Var { name: "SsmPol",  section: "Miscellaneous", desc: "Sheet Set Manager poll time", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Stub, default: "60", wired: false },
    Var { name: "SsmSta",  section: "Miscellaneous", desc: "Sheet Set Manager status", kind: Kind::Bool, status: Status::Stub, default: "false", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // RUST_CAD-specific
    // ───────────────────────────────────────────────────────────────────
    Var { name: "GpuRnd",  section: "RUST_CAD-specific", desc: "Rendering path: CPU / GPU-auto / GPU-forced", kind: Kind::Choice(&["CPU", "GPU-auto", "GPU-forced"]), status: Status::Planned, default: "1", wired: false },
    Var { name: "FpsDsp",  section: "RUST_CAD-specific", desc: "FPS overlay visibility", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "IdxDsp",  section: "RUST_CAD-specific", desc: "Spatial-index status overlay", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "IdxCel",  section: "RUST_CAD-specific", desc: "Spatial-index target cells per dobject", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "BgCol",   section: "RUST_CAD-specific", desc: "Canvas background colour", kind: Kind::Color, status: Status::Planned, default: "0x12161C", wired: false },
    Var { name: "SnpPri",  section: "RUST_CAD-specific", desc: "Snap priority order", kind: Kind::Text, status: Status::Planned, default: "end,mid,cen,int", wired: false },
    Var { name: "SnpAct",  section: "RUST_CAD-specific", desc: "Default SnapSet at startup", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "255", wired: false },
    Var { name: "TabCyc",  section: "RUST_CAD-specific", desc: "Tab cycling between snap candidates", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "CmdEcho", section: "RUST_CAD-specific", desc: "Echo commands to history", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "CmdHisM", section: "RUST_CAD-specific", desc: "Command history retention size", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "500", wired: false },
    Var { name: "RubBnd",  section: "RUST_CAD-specific", desc: "Rubber-band style", kind: Kind::Choice(&["solid", "dashed", "animated"]), status: Status::Planned, default: "0", wired: false },
    Var { name: "MvDdsp",  section: "RUST_CAD-specific", desc: "Move-tool ghost render style", kind: Kind::Choice(&["ghost", "outline", "off"]), status: Status::Planned, default: "0", wired: false },
    Var { name: "RsmCmp",  section: "RUST_CAD-specific", desc: ".rsm save format", kind: Kind::Choice(&["uncompressed", "LZ4", "zstd"]), status: Status::Planned, default: "1", wired: false },
    Var { name: "RsmBak",  section: "RUST_CAD-specific", desc: "Keep .rsm.bak on save", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Code-Audit Hardcoded
    // ───────────────────────────────────────────────────────────────────
    Var { name: "DefDClr",   section: "Code-Audit Hardcoded", desc: "Default dobject colour", kind: Kind::Color, status: Status::Planned, default: "0xAAC8E6", wired: false },
    Var { name: "SelClr",    section: "Code-Audit Hardcoded", desc: "Selected dobject highlight colour", kind: Kind::Color, status: Status::Planned, default: "0xFFC850", wired: false },
    Var { name: "SnpSrcClr", section: "Code-Audit Hardcoded", desc: "Snap-source entity highlight colour", kind: Kind::Color, status: Status::Planned, default: "0x78F0FF", wired: false },
    Var { name: "SnpClr",    section: "Code-Audit Hardcoded", desc: "Snap glyph + label colour", kind: Kind::Color, status: Status::Planned, default: "0x50E6F0", wired: false },
    Var { name: "IntClr",    section: "Code-Audit Hardcoded", desc: "Intersection marker colour (alias of IntsCol)", kind: Kind::Color, status: Status::Planned, default: "0xFF5A5A", wired: false },
    Var { name: "ExtClr",    section: "Code-Audit Hardcoded", desc: "Imaginary-extension dashed-line colour", kind: Kind::Color, status: Status::Planned, default: "0xFFC85A", wired: false },
    Var { name: "PreClr",    section: "Code-Audit Hardcoded", desc: "Preview / rubber-band colour", kind: Kind::Color, status: Status::Planned, default: "0xFFDC64", wired: false },
    Var { name: "ExtSpd",    section: "Code-Audit Hardcoded", desc: "Extension-dash drift speed (px/sec)", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "60", wired: false },
    Var { name: "ExtFade",   section: "Code-Audit Hardcoded", desc: "Extension-dash alpha base", kind: Kind::Float { min: 0.0, max: 1.0 }, status: Status::Planned, default: "0.55", wired: false },
    Var { name: "ExtDshL",   section: "Code-Audit Hardcoded", desc: "Extension-dash length (px)", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "7", wired: false },
    Var { name: "ExtGapL",   section: "Code-Audit Hardcoded", desc: "Extension-dash gap (px)", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "4", wired: false },
    Var { name: "WinDshSpd", section: "Code-Audit Hardcoded", desc: "Selection-window dash drift speed", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "40", wired: false },
    Var { name: "SelDshClr", section: "Code-Audit Hardcoded", desc: "Selection-basket dashed overlay colour", kind: Kind::Color, status: Status::Planned, default: "0xB4D2E6", wired: false },
    Var { name: "SelDshW",   section: "Code-Audit Hardcoded", desc: "Selection-basket dashed overlay stroke width", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "1.6", wired: false },
    Var { name: "SelDshL",   section: "Code-Audit Hardcoded", desc: "Selection-basket dash length", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "6", wired: false },
    Var { name: "SelDshG",   section: "Code-Audit Hardcoded", desc: "Selection-basket dash gap", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "4", wired: false },
    Var { name: "SelPlsMin", section: "Code-Audit Hardcoded", desc: "Selection-basket pulse alpha min", kind: Kind::Float { min: 0.0, max: 1.0 }, status: Status::Planned, default: "0.15", wired: false },
    Var { name: "SelPlsMax", section: "Code-Audit Hardcoded", desc: "Selection-basket pulse alpha max", kind: Kind::Float { min: 0.0, max: 1.0 }, status: Status::Planned, default: "0.85", wired: false },
    Var { name: "SelPlsHz",  section: "Code-Audit Hardcoded", desc: "Selection-basket pulse frequency", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "1.4", wired: false },
    Var { name: "HitTolPx",  section: "Code-Audit Hardcoded", desc: "Hit-test tolerance (pixels) (overlaps PkBxSz)", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "IntRad",    section: "Code-Audit Hardcoded", desc: "∩ click search radius", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "50", wired: false },
    Var { name: "PairLim",   section: "Code-Audit Hardcoded", desc: "Max candidate pair count", kind: Kind::Int { min: 0, max: 1_000_000_000 }, status: Status::Planned, default: "5000000", wired: false },
    Var { name: "TabCycR",   section: "Code-Audit Hardcoded", desc: "Cursor-move px before Tab cycle resets", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "4", wired: false },
    Var { name: "ArrCol",    section: "Code-Audit Hardcoded", desc: "Array default columns", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "ArrRow",    section: "Code-Audit Hardcoded", desc: "Array default rows", kind: Kind::Int { min: 0, max: 1_000_000 }, status: Status::Planned, default: "10", wired: false },
    Var { name: "ArrDX",     section: "Code-Audit Hardcoded", desc: "Array delta X", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "50", wired: false },
    Var { name: "ArrDY",     section: "Code-Audit Hardcoded", desc: "Array delta Y", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "50", wired: false },
    Var { name: "DfltZm",    section: "Code-Audit Hardcoded", desc: "Default zoom scale", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "6", wired: false },
    Var { name: "DemoOn",    section: "Code-Audit Hardcoded", desc: "Load demo dobjects on startup", kind: Kind::Bool, status: Status::Planned, default: "true", wired: false },
    Var { name: "GpuRgWd",   section: "Code-Audit Hardcoded", desc: "GPU circle ring thickness", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "1", wired: false },
    Var { name: "TessCirc",  section: "Code-Audit Hardcoded", desc: "Circle CPU tessellation factor", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "0.5", wired: false },
    Var { name: "TessArc",   section: "Code-Audit Hardcoded", desc: "Arc CPU tessellation factor", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "0.5", wired: false },
    Var { name: "TessEll",   section: "Code-Audit Hardcoded", desc: "Ellipse tessellation factor", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "0.7", wired: false },
    Var { name: "TessEArc",  section: "Code-Audit Hardcoded", desc: "EllipseArc tessellation factor", kind: Kind::Float { min: -1e9, max: 1e9 }, status: Status::Planned, default: "0.7", wired: false },

    // ───────────────────────────────────────────────────────────────────
    // Grid & CARD  (wired UserEnv fields, not in the AutoCAD catalog)
    // ───────────────────────────────────────────────────────────────────
    Var { name: "GrdEnb",  section: "Grid & CARD", desc: "Background grid display (GRIDMODE)", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
    Var { name: "GrdSnp",  section: "Grid & CARD", desc: "Snap cursor to grid intersections (SNAPMODE)", kind: Kind::Bool, status: Status::Active, default: "false", wired: true },
    Var { name: "GrdSpc",  section: "Grid & CARD", desc: "Grid spacing in world units (GRIDUNIT)", kind: Kind::Float { min: 0.0001, max: 1e9 }, status: Status::Active, default: "10", wired: true },
    Var { name: "CrdEnb",  section: "Grid & CARD", desc: "CARD cardinal-directions drafting lock", kind: Kind::Bool, status: Status::Active, default: "false", wired: true },

    // ───────────────────────────────────────────────────────────────────
    // UCS Icon  (wired UserEnv fields, not in the AutoCAD catalog)
    // ───────────────────────────────────────────────────────────────────
    Var { name: "UcsIcn",  section: "UCS Icon", desc: "UCS indicator on/off (UCSICON)", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
    Var { name: "UcsMod",  section: "UCS Icon", desc: "UCS icon placement mode", kind: Kind::Choice(&["corner", "origin"]), status: Status::Active, default: "0", wired: true },
    Var { name: "UcsAvP",  section: "UCS Icon", desc: "Path to UCS X-axis avatar image", kind: Kind::Text, status: Status::Active, default: "", wired: true },

    // ───────────────────────────────────────────────────────────────────
    // Drafting Defaults  (wired UserEnv fields, not in the AutoCAD catalog)
    // ───────────────────────────────────────────────────────────────────
    Var { name: "FltRad",  section: "Drafting Defaults", desc: "Default fillet radius (FILLETRAD)", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Active, default: "0", wired: true },
    Var { name: "ChmDs1",  section: "Drafting Defaults", desc: "Default chamfer distance, first line (CHAMFERA)", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Planned, default: "0", wired: true },
    Var { name: "ChmDs2",  section: "Drafting Defaults", desc: "Default chamfer distance, second line (CHAMFERB)", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Planned, default: "0", wired: true },
    Var { name: "OfsDis",  section: "Drafting Defaults", desc: "Default offset distance (OFFSETDIST)", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Active, default: "1", wired: true },
    Var { name: "WlThk",   section: "Drafting Defaults", desc: "Default wall thickness", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Active, default: "0.2", wired: true },
    Var { name: "TxHt",    section: "Drafting Defaults", desc: "Default text height (world units)", kind: Kind::Float { min: 0.0, max: 1e9 }, status: Status::Active, default: "0.25", wired: true },
    Var { name: "WlCnL",   section: "Drafting Defaults", desc: "Wall centerline visible", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
    Var { name: "TrmMd",   section: "Drafting Defaults", desc: "Trim mode shared by Fillet and Chamfer (TRIMMODE)", kind: Kind::Bool, status: Status::Active, default: "true", wired: true },
];

/// Unique section names, in first-appearance order.
pub fn sections() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for v in VARS { if !out.contains(&v.section) { out.push(v.section); } }
    out
}

pub fn find(name: &str) -> Option<&'static Var> {
    VARS.iter().find(|v| v.name.eq_ignore_ascii_case(name))
}

// ── value parsing / formatting helpers ──────────────────────────────────

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Ok(true),
        "false" | "0" | "off" | "no" => Ok(false),
        _ => Err(format!("'{}' is not a boolean (expected on/off, true/false, 1/0)", s)),
    }
}

fn bool_disp(v: bool) -> String { if v { "on".to_string() } else { "off".to_string() } }

fn parse_color(s: &str) -> Result<u32, String> {
    let t = s.trim();
    // r,g,b form
    if t.contains(',') {
        let parts: Vec<&str> = t.split(',').map(|p| p.trim()).collect();
        if parts.len() != 3 {
            return Err(format!("'{}' is not a colour (expected r,g,b or #RRGGBB)", s));
        }
        let mut rgb = 0u32;
        for (i, p) in parts.iter().enumerate() {
            let c: u32 = p.parse().map_err(|_| format!("'{}' has a bad colour component '{}'", s, p))?;
            if c > 255 { return Err(format!("colour component '{}' is out of range 0..255", p)); }
            rgb |= c << (8 * (2 - i));
        }
        return Ok(rgb);
    }
    let hex = t
        .strip_prefix("0x").or_else(|| t.strip_prefix("0X"))
        .or_else(|| t.strip_prefix('#'))
        .unwrap_or(t);
    let v = u32::from_str_radix(hex, 16)
        .map_err(|_| format!("'{}' is not a colour (expected #RRGGBB or 0xRRGGBB)", s))?;
    Ok(v & 0x00FF_FFFF)
}

fn color_disp(v: u32) -> String { format!("0x{:06X}", v & 0x00FF_FFFF) }

fn parse_choice(names: &[&str], s: &str) -> Result<u8, String> {
    let t = s.trim();
    // numeric index?
    if let Ok(idx) = t.parse::<usize>() {
        if idx < names.len() {
            return Ok(idx as u8);
        }
        return Err(format!("index {} out of range 0..{}", idx, names.len() - 1));
    }
    // option word (case-insensitive)
    for (i, n) in names.iter().enumerate() {
        if n.eq_ignore_ascii_case(t) {
            return Ok(i as u8);
        }
    }
    Err(format!("'{}' is not one of {:?}", s, names))
}

/// Read a WIRED variable's current value as a display string. None if the
/// name isn't a wired UserEnv field.
pub fn env_get(env: &UserEnv, name: &str) -> Option<String> {
    let v = find(name)?;
    if !v.wired { return None; }
    // Choice option-word lookup helper.
    let choice = |idx: u8| -> String {
        if let Kind::Choice(names) = v.kind {
            names.get(idx as usize).map(|s| s.to_string()).unwrap_or_else(|| idx.to_string())
        } else {
            idx.to_string()
        }
    };
    let out = match v.name {
        // U8
        "SpTGSZ" => env.SpTGSZ.to_string(),
        "PkBxSz" => env.PkBxSz.to_string(),
        "CrsHrS" => env.CrsHrS.to_string(),
        "GrpSz"  => env.GrpSz.to_string(),
        "GrpHvR" => env.GrpHvR.to_string(),
        // Bool
        "AtDlgM" => bool_disp(env.AtDlgM),
        "AtPrmM" => bool_disp(env.AtPrmM),
        "CmDlgM" => bool_disp(env.CmDlgM),
        "FlDlgM" => bool_disp(env.FlDlgM),
        "EdgMod" => bool_disp(env.EdgMod),
        "WlCnL"  => bool_disp(env.WlCnL),
        "TrmMd"  => bool_disp(env.TrmMd),
        "MnuBar" => bool_disp(env.MnuBar),
        "TltEnb" => bool_disp(env.TltEnb),
        "RllTp"  => bool_disp(env.RllTp),
        "SelPrv" => bool_disp(env.SelPrv),
        "HltSel" => bool_disp(env.HltSel),
        "GrpEnb" => bool_disp(env.GrpEnb),
        "GrpBlk" => bool_disp(env.GrpBlk),
        "GrdEnb" => bool_disp(env.GrdEnb),
        "GrdSnp" => bool_disp(env.GrdSnp),
        "CrdEnb" => bool_disp(env.CrdEnb),
        "UcsIcn" => bool_disp(env.UcsIcn),
        // Float
        "FltRad" => env.FltRad.to_string(),
        "ChmDs1" => env.ChmDs1.to_string(),
        "ChmDs2" => env.ChmDs2.to_string(),
        "OfsDis" => env.OfsDis.to_string(),
        "WlThk"  => env.WlThk.to_string(),
        "TxHt"   => env.TxHt.to_string(),
        "GrdSpc" => env.GrdSpc.to_string(),
        // Int
        "SelDmTm" => env.SelDmTm.to_string(),
        // Choice
        "DrDspM" => choice(env.DrDspM),
        "WpFrmM" => choice(env.WpFrmM),
        "XrLdMd" => choice(env.XrLdMd),
        "UcsMod" => choice(env.UcsMod),
        "LodAnc" => choice(env.LodAnc),
        // Color
        "GrClrU" => color_disp(env.GrClrU),
        "GrClrS" => color_disp(env.GrClrS),
        // Text
        "UcsAvP" => env.UcsAvP.clone(),
        "XrTmpP" => env.XrTmpP.clone(),
        _ => return None,
    };
    Some(out)
}

/// Validate + clamp + set a WIRED variable from a string. Err(msg) on bad
/// input or unknown/unwired name. This is the ONE setter the UI, the command
/// line, and the file loader should all funnel through.
pub fn env_set(env: &mut UserEnv, name: &str, val: &str) -> Result<(), String> {
    let Some(v) = find(name) else {
        return Err(format!("unknown variable '{}'", name));
    };
    if !v.wired {
        return Err(format!(
            "variable '{}' is not editable yet (status: {:?})",
            v.name, v.status
        ));
    }

    match v.kind {
        Kind::Bool => {
            let b = parse_bool(val)?;
            set_bool(env, v.name, b);
        }
        Kind::U8 { min, max } => {
            let n: i64 = val.trim().parse()
                .map_err(|_| format!("'{}' is not an integer", val))?;
            let c = n.clamp(min as i64, max as i64) as u8;
            set_u8(env, v.name, c);
        }
        Kind::Int { min, max } => {
            let n: i64 = val.trim().parse()
                .map_err(|_| format!("'{}' is not an integer", val))?;
            let c = n.clamp(min, max);
            set_int(env, v.name, c);
        }
        Kind::Float { min, max } => {
            let f: f64 = val.trim().parse()
                .map_err(|_| format!("'{}' is not a number", val))?;
            if f.is_nan() { return Err(format!("'{}' is not a number", val)); }
            let c = f.clamp(min, max);
            set_float(env, v.name, c);
        }
        Kind::Choice(names) => {
            let idx = parse_choice(names, val)?;
            set_u8(env, v.name, idx);
        }
        Kind::Color => {
            let rgb = parse_color(val)?;
            set_color(env, v.name, rgb);
        }
        Kind::Text => {
            set_text(env, v.name, val);
        }
    }
    Ok(())
}

// ── per-kind writers (the wired field map) ──────────────────────────────

fn set_bool(env: &mut UserEnv, name: &str, b: bool) {
    match name {
        "AtDlgM" => env.AtDlgM = b,
        "AtPrmM" => env.AtPrmM = b,
        "CmDlgM" => env.CmDlgM = b,
        "FlDlgM" => env.FlDlgM = b,
        "EdgMod" => env.EdgMod = b,
        "WlCnL"  => env.WlCnL = b,
        "TrmMd"  => env.TrmMd = b,
        "MnuBar" => env.MnuBar = b,
        "TltEnb" => env.TltEnb = b,
        "RllTp"  => env.RllTp = b,
        "SelPrv" => env.SelPrv = b,
        "HltSel" => env.HltSel = b,
        "GrpEnb" => env.GrpEnb = b,
        "GrpBlk" => env.GrpBlk = b,
        "GrdEnb" => env.GrdEnb = b,
        "GrdSnp" => env.GrdSnp = b,
        "CrdEnb" => env.CrdEnb = b,
        "UcsIcn" => env.UcsIcn = b,
        _ => {}
    }
}

fn set_u8(env: &mut UserEnv, name: &str, n: u8) {
    match name {
        "SpTGSZ" => env.SpTGSZ = n,
        "PkBxSz" => env.PkBxSz = n,
        "CrsHrS" => env.CrsHrS = n,
        "GrpSz"  => env.GrpSz = n,
        "GrpHvR" => env.GrpHvR = n,
        // Choice fields are stored as u8 indices
        "DrDspM" => env.DrDspM = n,
        "WpFrmM" => env.WpFrmM = n,
        "XrLdMd" => env.XrLdMd = n,
        "UcsMod" => env.UcsMod = n,
        "LodAnc" => env.LodAnc = n,
        _ => {}
    }
}

fn set_int(env: &mut UserEnv, name: &str, n: i64) {
    match name {
        "SelDmTm" => env.SelDmTm = n as u16,
        _ => {}
    }
}

fn set_float(env: &mut UserEnv, name: &str, f: f64) {
    match name {
        "FltRad" => env.FltRad = f,
        "ChmDs1" => env.ChmDs1 = f,
        "ChmDs2" => env.ChmDs2 = f,
        "OfsDis" => env.OfsDis = f,
        "WlThk"  => env.WlThk = f,
        "TxHt"   => env.TxHt = f,
        "GrdSpc" => env.GrdSpc = f,
        _ => {}
    }
}

fn set_color(env: &mut UserEnv, name: &str, rgb: u32) {
    match name {
        "GrClrU" => env.GrClrU = rgb,
        "GrClrS" => env.GrClrS = rgb,
        _ => {}
    }
}

fn set_text(env: &mut UserEnv, name: &str, s: &str) {
    match name {
        "UcsAvP" => env.UcsAvP = s.to_string(),
        "XrTmpP" => env.XrTmpP = s.to_string(),
        _ => {}
    }
}
