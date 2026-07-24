# 2D Drafting Parity — sandbox vs RUST_CAD (the gate to 3D)

**Rule:** complete 2D drafting (draw + edit + snap + input) in the flat sketch, at
RUST_CAD parity, BEFORE extrude/boolean work. The geometry engines are ALREADY in
the shared `cad_kernel` (byte-identical to RUST_CAD) — this is interaction wiring,
not new math. Each row names the kernel function to reuse. **Never reimplement the
geometry.**

Canonical specs: `mentor MD/BASIC_MODIFIERS_RULES.md` (modifiers), `PLINE_GUIDE.md`
(polyline). Read those first.

---

## DRAW  (create geometry)
| Command | Kernel | Status |
|---|---|---|
| Line (chained) | `Line` | ✅ done |
| Polyline: Line/Arc mode, tangent bulges, C/U, auto-close | `PolyVertex.bulge`, `join::bulge_arc` | ✅ done |
| Rectangle | rect→closed polyline | ✅ done |
| Circle: center-radius / diameter / 2P / 3P | `Circle`, `arc_three_points` | ✅ done |
| Arc: 3-point / start-center-end / center-start-end | `arc_three_points`, `arc_center_start_end` | ✅ done |
| Ellipse: center-major-minor / axis-ends | `ellipse_center_major_minor` | ✅ done |
| Point | `Point` | ✅ done |
| Pline WIDTH (w/h taper, sticky) | `Polyline.widths` | ⬜ deferred |
| Pline 3-point-arc (s) / direction (d) | `join::bulge_from_arc` | ⬜ deferred |
| Spline | `Spline::new_bspline` + tessellation | ⬜ TODO (needs geom_outlines case) |

## MODIFY  (edit geometry)
| Command | Kernel | Status |
|---|---|---|
| Move / Copy | `DObject::translated` | ✅ done |
| Rotate / Scale | `DObject::rotated` / `scaled` | ✅ done |
| Mirror (keep original) | `DObject::mirrored` | ✅ done |
| Erase | `Document` remove | ✅ done |
| **Offset** | `Geom::offset(dist, side)` | ✅ done — `offset` → dist → pick obj → pick side |
| **Trim** | `Geom::trim_at` | ✅ done — `trim` → click piece (all others = cutters), repeats |
| **Extend** | `Geom::extend_to` | ✅ done — `extend` → click end (all others = boundaries) |
| **Fillet** | `fillet_geoms` | ✅ done — `fillet` → radius → pick 2 objects |
| **Chamfer** | `chamfer_geoms` | ✅ done — `chamfer` → dist → pick 2 objects |
| **Join** | `join_geoms` | ✅ done — select 2+ → `join` |
| **Break** | `Geom::split_at` | ✅ done — `break` → pick obj → 2 points |
| Array (rect/polar) | transforms in a loop | ⬜ TODO (needs count/spacing UI) |

## SNAP / INPUT  (precision)
| Feature | Kernel | Status |
|---|---|---|
| Osnap END/MID/CEN/QUA over drawn geom + face ref | `snap::find_snap` | ✅ done |
| Osnap INT / PER / TAN / NEA | same (`SnapKind::Int/Per/Tan/Nea`) | ⬜ TODO — wire kinds + `from` anchor |
| **Phantom snap** (snap to the IN-PROGRESS pline vertices) | feed pending as a temp dobject to `find_snap` | ⬜ NEXT — needed for auto-close/chaining |
| Absolute `x,y` | — | ✅ done |
| Relative `@dx,dy` / Polar `@d<a` | — | ✅ done |
| Direct distance entry (type dist, cursor dir) | — | ⬜ TODO |
| CARD / ORTHO (H/V lock) | project delta to axis | ⬜ NEXT |
| Grid snap | round to grid | ⬜ TODO |
| Inline snap override (type END/MID mid-command) | forced kind on `find_snap` | ⬜ TODO |

## SELECTION
| Feature | Status |
|---|---|
| Single click select (nearest) | ✅ done |
| Shift add/toggle | ✅ done |
| Window / crossing drag (L→R / R→L) | ⬜ NEXT |
| all / prev / none sub-commands | ⬜ TODO |

## DOCUMENT
| Feature | Status |
|---|---|
| Plane-linked sketch (reuse on re-entry) | ✅ done |
| Undo / redo of the sketch doc | ⬜ NEXT |
| Layers / linetypes / colors on sketch geom | ⬜ TODO |

---

## Priority order to reach "usable like RUST_CAD"
1. **Offset + Trim + Extend + Fillet/Chamfer** (the editing core — all kernel-backed).
2. **Phantom snap** (snap to in-progress pline) + **CARD** (ortho) — precision while drawing.
3. **Window/crossing selection** + **Undo/redo**.
4. INT/PER/TAN/NEA snaps, direct-distance entry, break/join/array.
5. Deferred pline (width/3pt-arc/direction), spline.

Then S4 extrude (closed profile → CSG prism) → boolean.
