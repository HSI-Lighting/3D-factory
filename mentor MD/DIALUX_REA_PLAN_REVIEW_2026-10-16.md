# Review — "Reverse-engineer DIALux in pure Rust" (REA plan)

**Version:** v1.0 · **Date:** 2026-10-16 · **Status:** review of an external plan; nothing built yet · **Maintainer:** mentor

The owner brought in a generic plan: reverse-engineer DIALux through its file format, its UI
(UI Automation), and its calculation behaviour, then rebuild it in Rust over five phases
(about 24–31 weeks). This review says what to keep, what to cut, and why. Every factual
claim about DIALux below was checked on a real file on this machine, not taken from the plan.

---

## 0. Verdict

**As a plan for SIMLUX: no. About 70 % of it rebuilds things SIMLUX already has, or aims at
copying the app instead of meeting the standard.** It reads as if written for someone starting
from nothing. SIMLUX is not starting from nothing: M0–M8 are done, the hybrid
direct-raytrace + radiosity engine is locked and matches DIALux within 0.50 % on identical
rooms, and multi-room calculation, UGR, the report generator and the DWG converter all exist.

The plan also contradicts the locked reframe in `SIMLUX_DIALUX_PLAN.md` §0: *"Build to the
standard DIALux implements, not to the app. Chasing the app never converges."* Its UI
Automation bridge and the step "replicate DIALux's menu structure and dialog flows" are
exactly the app-chasing that §0 rules out.

**One part is new and worth doing: importing `.evo` files.** The plan's central fact turned
out half right, and the half it got wrong changes the design (§1).

---

## 1. What a `.evo` file actually is (verified 2026-10-16)

Inspected: `D:\Dropbox\03--PROJECTS\03-PROJECTS\2023\08-others\02-DIALUX -ROUGH\Project 0.evo`
(saved by DIALux 5.10.1.58791; the installed DIALux is 5.14.0.5).

| Entry | What it is |
|---|---|
| the file itself | a **ZIP archive** (24 entries), not a STEP file |
| `Project/ProjectData/ProjectData.dat` | **ISO-10303-21 syntax**, but `FILE_SCHEMA(('DIALux {5.10.1.58791}'))`, which is DIALux's own private schema and not an ISO application protocol. Only the tokenizer is standard; no STEP-AP library helps. |
| `Project/ProjectData/ProjectData.xml` | **the schema**: `<MetaInfo Version="5.10.1.58791"><PersistenceObject Name=…><Member Name=… IndexInObject=…>`. It ships **inside every project file**. It is *not* in the install folder: `C:\Program Files\DIAL GmbH\DIALux` contains no `ProjectData*.xml` and no `.xsd`, so the plan's "extract it from the installation directory" is wrong. |
| `Project/ScenegraphScene` | a **boost::serialization binary archive** (starts with `serialization::archive`), 358 KB, larger than the project data. **The plan never mentions it.** It is not self-describing and is tied to class versions. |
| `Project/Data/<guid>` | opaque blobs (empty in this file). Probably embedded photometry or geometry; check this on a file that contains luminaires. |
| `Project/Results/ProjectToc.rsl` | stored results (empty here because the file was never calculated) |

The entity names in `ProjectData.dat` follow **IFC's object graph**: `RelAggregates`,
`RelContainedInSpatialStructure`, `RelDefinesByProperties`, `RelFillsElement`,
`BuildingOpeningElement`, `StoreyContour`, `MultiPolygonSurface2D`, `CoordSys3D`,
`MaterialParams`, `ProductData`. Anyone who has read IFC will find their way around it.

### Three design consequences

1. **Generating code from the schema is the wrong tool.** The schema travels with each file
   and changes with each version (this file is 5.10, the install is 5.14). Rust structs
   generated at build time from one version's XML break on the next version. The right design
   reads `ProjectData.xml` **at run time**: for each `PersistenceObject`, build a map from
   member name to `IndexInObject`, then read fields by name. Store the entities as a generic
   value tree (`Ref / Int / Real / Str / Enum / List / Null`) in a `Vec` with an id→index map,
   and hand-write extractors for only the roughly ten types SIMLUX needs. That is hundreds of
   lines, not "4–6 weeks of codegen".
2. **Import only. No serializer and no round-trip writer.** Writing a `.evo` that DIALux
   accepts means honouring invariants nobody has documented: the counts in the
   `ObjectManager` at `#1`, `PropertySetProxyCache`, consistency with `ScenegraphScene`. The
   failure mode is a client's DIALux refusing a file, and there is no business case until
   someone needs to send work back to DIALux. (A tokenizer test that parses, re-emits and
   parses again is fine; it tests the tokenizer, not a writer.)
3. **No `Arc<Mutex<>>` graph.** The data is parsed once and only read afterwards. A plain
   `Vec` plus an index map is enough.

**The open question that decides the rest:** is enough geometry in `ProjectData.dat`
(`StoreyContour` + `MultiPolygonSurface2D` + storey and room heights suggest the 2-D contours
are there), or do rooms and furniture exist only in `ScenegraphScene`? If the second, the
real reverse-engineering cost is the boost archive, and the plan does not budget for it.

---

## 2. What to cut, and why

| Plan item | Cut because |
|---|---|
| UI Automation bridge / inspector / recorder (`automation/`) | Copying the UI is app-chasing (§0 of the DIALux plan). It is also the part most exposed to look-and-feel claims. DIALux is WPF/.NET, so its UIA tree is huge and noisy; screenshots of the dozen dialogs you care about give the same information in an afternoon. |
| "Replicate DIALux's menu structure and dialog flows" | Same reason. SIMLUX has its own command and menu system (`COMMAND_REGISTRY_MENTOR.md`, `MENU_DROPDOWN_MENTOR.md`). |
| Phase 3 calculation engine ("radiosity + ray-traced direct", "ALC room partitioning") | Already built and locked (Decision 2). "Room-by-room" is the existing multi-room `RoomResult` path, which runs from one shared Evaluator. Rebuilding it throws away the 0.50 % agreement. |
| Bevy 3-D viewport | It would be a second renderer, and an ECS fighting the app's architecture. SIMLUX already has a wgpu renderer with sRGB→linear→AgX, GGX shading, shadow maps and a Radiance export. |
| `parry3d`, `eulumdat-rs` | `cad_light` already parses LDT and IES and does its own ray–scene queries. Only look at **`gldf-rs`**, and only if GLDF catalogue import becomes a requirement. That would be a real gap; the rest of this list is not. |
| Schema→Rust codegen, serializer, round-trip | See §1. |
| Cross-platform packaging | Nobody has asked for macOS or Linux. Add it when someone does. |

---

## 3. What the plan gets wrong about the oracle

The plan treats DIALux as the ground truth and proposes reverse-engineering its algorithm.
Both halves are off:

- **The primary oracle is CIE 171:2006**, the standard set of analytical test cases for
  lighting software, which DIALux itself is validated against. DIALux is the *second* oracle,
  and SIMLUX already has one (`cad_light/tests/identical_dialux`).
- The plan names none of the things that actually make numbers diverge between programs:
  maintenance factor, the EN 12464-1 grid rule (`p = 0.2 × 5^log10(d)`), the border band,
  measurement height, LOR and flux scaling, and which UGR method is used. Most of SIMLUX's
  v0.1–v0.3 work was there, not in the solver.

**The one legitimate use of driving DIALux's UI is as a batch oracle.** A script opens each
`.evo`, runs the calculation and exports the results, which mass-produces validation pairs.
That is a later phase, and a pywinauto or AutoHotkey script does the job; it needs no Rust
UIA bridge.

---

## 4. Legal posture (not legal advice, but it shapes the method)

DIAL GmbH is German, so the EU Software Directive 2009/24/EC applies alongside the EULA.

- **Art. 5(3):** observing and testing the program while running it legitimately is allowed.
  Black-box comparison of results is fine.
- **Art. 6:** decompiling is allowed **only** to achieve interoperability with an
  independently created program, only for the parts needed for that, and **never** to build a
  substantially similar program. DIALux is managed .NET (407 DLLs, `*.deps.json`), so ILSpy
  would make decompiling trivial. **Do not do it.** Learning the file format from the
  company's **own** project files, as in §1, is the clean route, and `.evo` import into SIMLUX
  is the textbook interoperability case.
- Read the DIALux EULA before any automation step.

---

## 5. Found while checking: the 0.50 % gate is currently unenforced

`D:\Dropbox\YASEEN\3d factory\tests\Identical testing\` (holding `FONDO.ldt`, `t1/t2/t3`
`.evo` / `.pdf` / `.simlux.json` and the validation write-ups) **is in the D: Recycle Bin**.
Windows recorded the deletion on 2026-09-10. A search of D:, G: and E: found no other copy
of `t1.evo`. Without that path the `identical_dialux` tests skip silently and report success,
so every `cad_light` change since then has passed the gate without being checked against
DIALux. **Restore it first.** It is also the corpus for the `.evo` import gate below.

---

## 6. Phased path (replaces the plan's five phases)

**Phase 0 — one hour.** Restore `Identical testing`. Open `t1.evo` (which has a DIALux PDF
and a `.simlux.json` beside it) and list where each of these lives: room polygon, height,
openings, surface reflectances, luminaire position, rotation, photometry and maintenance
factor, either in `ProjectData.dat` or in `ScenegraphScene` / `Data/<guid>`. That answers the
§1 open question.

**Phase 1 — about a week. `.evo` reader.** ZIP + Part-21 tokenizer → generic entity table +
run-time schema from `ProjectData.xml`. Gates:
- a corpus test (`#[ignore]`, behind a path environment variable like the identical tests)
  that parses every `.evo` under `D:\Dropbox\03--PROJECTS` without error, across all DIALux
  versions present;
- a deliberately truncated or corrupted file must produce an error, not a panic or an empty
  scene. Prove the test can fail before trusting it.

**Phase 2 — extractors → SIMLUX scene.** Rooms, openings, materials→reflectance, luminaires.
**The gate is independent of the importer:** import `t1.evo`, calculate, and land within
0.50 % of `t1.pdf`. The PDF was produced by DIALux, not by the importer, so this check cannot
pass for circular reasons (see the verification-independence rule).

**Phase 3 — only if Phase 0 shows it is needed.** `ScenegraphScene` for furniture meshes.
The default is to skip it: re-import furniture as glTF, which SIMLUX already handles.

**Later, optional.** DIALux batch oracle (§3) to grow the validation corpus.

---

## 7. Open questions for the author

1. **What is the goal?** (a) Bring existing DIALux projects into SIMLUX, (b) stop needing
   DIALux in-house, or (c) a commercial competitor? The answer changes the legal posture and
   the scope. This review assumes (a)/(b).
2. Does anyone need to **send work back** as `.evo`? If not, the writer stays cut for good.
3. Was `Identical testing` deleted on purpose (moved elsewhere?) or by accident?
