# SIMLUX — the UGR MAP: glare as a property of the space

**Version:** v1.0 · **Date:** 2026-08-21 · **Status:** plan, not yet built · **Maintainer:** mentor
**Touches:** `cad_light/src/ugr.rs`, `cad_light/src/calc.rs`, new `cad_light/src/ugr_map.rs`,
`cad_app/src/light.rs`, `cad_app/src/app.rs` (overlay), `cad_app/src/report/layout.rs`
**Companion:** `SIMLUX_DIALUX_PLAN.md` (§1 Decision 1 — UGR_L is on the EN 12464-1 checklist)

---

## 0. The misunderstanding, stated precisely

A client says *"this fitting is UGR<19"*. What they are quoting is **UGR_L**, the
**tabulated** figure from CIE 117: the luminaire dropped into a **standard room** —
a rectangle sized in multiples of the height above eye level, reflectances
0.70 / 0.50 / 0.20, a stated spacing-to-height ratio, one observer standing at the
mid-point of a wall at 1.2 m looking horizontally, crosswise and endwise. Every one
of the three DIALux reports in `tests/Identical testing` says so in its own footnote:
*"based on a rectangular space of 4.000 m × 4.000 m and SHR of 0.25."*

That number characterises **the fitting**. It is a fair way to compare two fittings.
It is not a statement about anybody's room.

The quantity the standard actually sets a limit on is the glare **an observer
experiences**, and that depends on:

| depends on | carried by the datasheet |
|---|---|
| where the observer is | ✗ |
| **which way they are looking** | ✗ |
| mounting height, spacing, **aim** | ✗ |
| room reflectances (they set the background) | ✗ |
| what is in the way — partitions, columns, soffits | ✗ |
| eye height: seated 1.2 m vs standing 1.6 m | ✗ |
| the luminous **aperture**, not the housing | partly |

So: **UGR is a field over (position, view direction), not a scalar on a product.**
The deliverable is the picture that makes that undeniable — a plan of the room
coloured by glare, with the seats that are fine, the seats that are not, and *which
way not to face*.

This matters most for exactly the case that prompted it: **aimed** luminaires. A
spot tipped 30° at a wall has a narrow beam; whether it glares is entirely a
question of whether a given eye is in that beam. The table method cannot express
that at all — its room has no aiming in it.

---

## 1. What already exists (and it is a good start)

`cad_light/src/ugr.rs` implements the **direct CIE 117 calculation** for one
observer: `ugr_at(observer, luminaires, profiles, background) -> Option<UgrResult>`,
with Levin's closed-form Guth position index, per-source working kept in
`UgrResult::sources`, and eight tests including a hand-checked worked example. Two
design calls in it are already right and must be preserved:

* **`None` is not `0`.** No source in view returns `None`. Zero would read as an
  excellent room. The map must carry this through — see §4.4.
* **A fitting with no declared aperture is *counted as skipped*** (`skipped_no_area`),
  not silently dropped. A UGR built from half the fittings looks like a pass.

What does **not** exist: any caller. `ugr_at` is exported from `lib.rs` and used by
one `#[ignore]`d test. Nothing in the app computes, stores, draws, or reports it.

---

## 2. Five defects that must be fixed BEFORE a map is drawn

A map multiplies whatever the point calculation does — including its errors, across
a thousand cells, in colour, on a page a client signs. Fix these first. Each was
found by reading `ugr.rs` against `calc.rs`, which already does the same job
correctly for illuminance.

### 2.1 — **Aim is ignored.** `ugr.rs:164`, `ugr.rs:176` — *the critical one*

```rust
let gamma_deg = (dir.z as f64).clamp(-1.0, 1.0).acos().to_degrees();   // world −Z
let azim = (dir.y.atan2(dir.x)).to_degrees() - l.rotation_deg;         // world +X
```

The photometric angles are read off the **world axes**. `calc.rs:185` reads them off
**the fitting's own frame**:

```rust
let (aim, c0, c90) = lum.frame();
let gamma = dir.dot(aim).acos()...;
let phi   = dir.dot(c90).atan2(dir.dot(c0))...;
```

`Luminaire::frame()` (`types.rs:181`) honours `tilt_deg`. `ugr.rs` does not, and
`tilt_deg` is precisely the field the aiming tool writes (`types.rs:88` — *"aiming
the light stays at the same height and at the same location, its place where its
pointed downward is what we are changing"*).

Consequence: **a tilted spotlight is read as if it still pointed at the floor.** The
engine samples the wrong C-plane and the wrong γ, so a 3000 cd beam pointed into
someone's eye is read as whatever the file holds at nadir — often near zero. The
glare map for an accent-lit space would be confidently, silently blank in the one
place it matters.

It bites twice, because `gamma_deg` is also what foreshortens the aperture at
`ugr.rs:165`: `projected_luminous_area(γ)` wants γ **from the aperture normal**,
which is the aim vector, not world-down. Fixing the frame fixes both.

**Fix.** The ray reaching the eye runs `−dir` (eye→luminaire, negated). So:
`γ = acos((−dir)·aim)`, `φ = atan2((−dir)·c90, (−dir)·c0)`. Untilted this reduces
to exactly today's expression — which is the regression test to write.

### 2.2 — **The candela multiplier is applied twice.** `ugr.rs:180`

```rust
let intensity = prof.intensity(gamma_deg, azim) * prof.multiplier * l.dimming;
```

`IesProfile::intensity` **already** applies it (`ies.rs:165`: `lerp(c0, c1, ht) * self.multiplier`).
EULUMDAT always parses `multiplier: 1.0` (`ldt.rs:177`), so this is invisible on `.ldt`
— but LM-63 reads a real value from the header (`ies.rs:239`), and IES files routinely
carry one. UGR goes as `I²`, so a file with multiplier 0.8 lands **1.5 UGR** low;
one with multiplier 2 lands 4.8 **high**. Both are inside the range that flips a
pass into a fail.

### 2.3 — **`flux_override` is ignored.** `ugr.rs:180`

`ugr.rs` scales by `l.dimming`; `calc.rs:188` scales by `lum.output_scale(prof)`,
which folds in `flux_override` (`types.rs:161`). A fitting the user re-rated to
2000 lm therefore **lights the room at one output and glares at another**. Use
`output_scale` — one accessor, so the two can never disagree again.

### 2.4 — **No occlusion.** `ugr.rs`, the loop at 144

Every luminaire in the list contributes if it is in the forward hemisphere. A
fitting behind a partition, above a soffit, or **in the next room** glares straight
through the wall. On a single-room test this never showed; on a real multi-room
model — which is the normal case here, see `RoomResult` (`light.rs:1055`) — it makes
the map wrong everywhere near a party wall, and wrong in the *unsafe* direction.

The machinery exists: `RtScene::occluded(from, to)` (`rt.rs:159`), already used by
`direct()` (`calc.rs:263`). It is one shadow ray per luminaire per observer point,
and — see §5 — it is **independent of view direction**, so it is paid once per point
and reused across all azimuths.

### 2.5 — **Maintenance is not applied.** structural

`Evaluator` carries a `maintenance` factor and applies it to everything it returns
(`calc.rs:38`). `ugr_at` takes raw luminaires and knows nothing about it. The lux
plan is maintained; a UGR quoted beside it would be day-one.

Note that **UGR is not invariant to a uniform output scale.** Scaling every
luminance by `m` scales the sum by `m²` and the background by `m`, so
`ΔUGR = 8·log₁₀(m)` — a 0.80 maintenance factor is **−0.78 UGR**. Small, real, and
it must be the same convention in both numbers on the page. Decide once, state it in
the report footnote: **quote UGR on the maintained installation**, consistent with
every other figure SIMLUX prints.

### 2.6 — while in there: `STANDING_EYE_M`

`ugr.rs:52` sets standing eye height to **1.5 m**. EN 12464-1 uses **1.2 m seated,
1.6 m standing**. Worth 0.1 m of check against your copy of the standard before it
is baked into two map layers and a report.

---

## 3. The refactor that makes a map affordable

Split `ugr_at` into the part that depends on **where the eye is** and the part that
depends on **which way it looks**. This is not tidying — it is a 16× saving, and it
keeps one formula so the map and the probe can never disagree.

```rust
/// One luminaire as seen from one eye point. Independent of view direction.
pub struct SourceView {
    pub id: u32,
    pub dir: Vec3,       // unit, eye -> luminaire
    pub luminance: f64,  // cd/m², I(toward eye) / A_projected
    pub omega: f64,      // sr, A_projected / d²
}

/// Everything visible from `eye`: photometry, aperture, ONE occlusion ray each.
pub fn sources_at(
    eye: Vec3,
    lums: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    visible: &dyn Fn(Vec3, Vec3) -> bool,   // Evaluator's occlusion, or always-true
) -> (Vec<SourceView>, usize /* skipped_no_area */);

/// The formula, from a prepared source list. Cheap: only `p` depends on `view`.
pub fn ugr_from(src: &[SourceView], view: Vec3, background: f64) -> Option<UgrResult>;

/// Unchanged public entry point, now a two-line composition of the above.
pub fn ugr_at(...) -> Option<UgrResult>;
```

`ugr_at` keeps its signature and its eight tests keep passing — that is the safety
property to assert first.

---

## 4. The map

### 4.1 The direction problem, and how to reduce it

UGR is a field over position **and** azimuth. A 2D map has to collapse the azimuth
axis, and *which collapse* changes what the picture means. Do not pick one — **store
the rose and derive all of them at draw time**, so the user can flip between
questions without re-running the calculation.

Sample **N = 16 azimuths** (22.5°) with the view **horizontal**, which is
EN 12464-1's observer. 16 matches the resolution of `semi_cylindrical`
(`calc.rs:130`) and costs nothing once §3 lands.

```rust
pub struct UgrGrid {
    pub cols: u32, pub rows: u32,
    pub dirs: u32,               // 16
    pub eye_height: f32,         // 1.2 or 1.6
    /// cols*rows*dirs, row-major then azimuth. NaN = unrated (see 4.4).
    pub values: Vec<f32>,
    /// Per cell: why it is unrated, if it is.
    pub status: Vec<CellStatus>,
    /// cd/m², per cell per direction — the background actually used.
    pub background: Vec<f32>,
}
```

Reductions, each a `Vec<f64>` over cells, all feeding the existing
`isolux::Field` / `trace` contouring (`isolux.rs:45`, `:123`) and the existing
overlay painter:

| map | question it answers | who wants it |
|---|---|---|
| **`max`** — worst azimuth | *"can anyone here be dazzled?"* | **default**; compliance |
| **`at(φ)`** — one fixed azimuth | *"the desks all face the screen wall"* | offices, classrooms — the real design case |
| `min` — best azimuth | *"can this seat be fixed by turning it?"* | design remedy |
| `mean` | orientation is arbitrary | circulation, atria |
| **`worst_azimuth`** — a **vector** field | *"do not sit facing **that way**"* | see 4.3 |

### 4.2 Bands, not a rainbow

Colour by the **EN 12464-1 UGR_L limits**, not a continuous ramp. The map's job is
to read as compliance at a glance:

```
<= 16   deep green   technical drawing, precision work
<= 19   green        offices, classrooms, reading, control rooms   <- the usual target
<= 22   amber        industrial, circulation, reception
<= 25   orange       rough work
 > 25   red
unrated  neutral grey, hatched
```

Reuse `report::options::ramp_rgb` (`options.rs:866`) with a stepped ramp so the plan
overlay (`app.rs:5205`), the 3D sheet and the report page stay one picture of one
field — the rule already enforced for lux (`app.rs:5220`).

### 4.3 The output that is actually actionable

Draw, on the plan, a **short arrow per cell pointing along `worst_azimuth`**, drawn
only where `max` exceeds the target. That converts a picture into an instruction:
*"this row of desks is fine as long as it does not face the window wall."* Nothing
else in the report says that, and it is the single thing a designer can act on
without moving a fitting.

### 4.4 `None` is not `0` — the trap to design against up front

`ugr_at` returns `None` for *no source in the field of view* and for *no background
to see them against*. A map that stores `0.0` for those paints them **deep green** —
"excellent" — when the truth is "unrated". That is the failure mode that would
discredit the whole feature.

Store `f32::NAN`, carry a per-cell reason, and paint unrated cells in a neutral
hatch that appears in the legend:

```rust
pub enum CellStatus {
    Rated,
    NoSourceInView,      // nothing forward of the observer
    NoBackground,        // L_b ~ 0 — unlit, or outside the room
    AllSkippedNoArea,    // fittings declare no aperture (see 4.5)
    OutOfDomain(Domain), // see 4.6
}
```

### 4.5 Fittings with no declared aperture

`skipped_no_area` already exists per point. Aggregate it to the map and **surface it
once, loudly**, above the legend: *"3 of 41 fittings declare no luminous area; they
are excluded from every figure on this map."* Do not invent an area from the housing
— `ies.rs:91` is explicit that a 600 mm fitting with a 300 mm aperture is four times
brighter than its outline suggests, and a fabricated area produces a fabricated UGR
that looks exactly like a real one.

### 4.6 Say when the answer is outside CIE 117's domain

CIE 117 assumes a **regular array of similar luminaires** in a rectangular room, with
each source subtending **0.0003 sr ≤ ω ≤ 0.1 sr**. An aimed-accent scheme — the case
that prompted this — routinely violates all three. Being honest about that is worth
more than a confident number, and it is cheap: ω is already computed per source.

* **ω < 0.0003 sr** (small sources — most downlights and spots at room distances):
  the standard's small-source substitution replaces `L²ω` with `200·I²/r²`, giving
  `UGR = 8·log₁₀[ (0.25/L_b)·Σ 200·I²/(r²p²) ]`. **Implement this branch** — it is a
  few lines and it is the branch aimed spots will land in. *Confirm the constant and
  the threshold against your copy of CIE 117 / CIE 147 before it ships.*
* **ω > 0.1 sr** (a large batten close overhead): partition the aperture into
  segments and sum each with **its own** position index — the source is too big for
  one `p` to describe. `segments_for` (`calc.rs:220`) is the existing precedent.
* **Non-uniform / aimed arrangement:** flag it in the report footnote. State that the
  figure is the direct CIE 117 calculation for the installation as built, and that
  no table value is comparable.

### 4.7 Heights

Two layers, both computed, toggled in the UI, both in the report:

* **Seated, 1.2 m** — offices, classrooms, meeting rooms, waiting areas.
* **Standing / walking, 1.6 m** *(pending §2.6)* — retail, circulation, workshops,
  kitchens, galleries.

They are genuinely different pictures: 0.4 m of eye height moves a ceiling fitting
several degrees closer to the line of sight, and the position index is steep there.
Do not compute one and offer the other as a caveat.

`LightState::eye_height` already exists (`light.rs:1175`, default 1.2, and it is in
the calculation fingerprint at `light.rs:876`) and drives the cylindrical figure.
Keep it as the *user's* eye height, and give the UGR map its own pair of standard
heights — the two questions are different and should not share a slider.

---

## 5. Cost — and why this is cheap

The expensive half of UGR is not the glare sum, it is the **background**.

**Glare sum — nearly free.** Per observer point: one occlusion ray per luminaire,
one photometric lookup, one projected area. **All of it is independent of view
direction** (§3), so the 16 azimuths afterwards are arithmetic — only `p` changes.
A 20 × 15 m room at 0.5 m cells is 1200 cells; two heights and 100 fittings is
**240 k shadow rays**, against the millions the lux grid already fires.

**Background — the real cost, and the trick.** `L_b = E_indirect,vertical(eye, φ)/π`
— the **indirect** illuminance on a vertical plane at the eye facing the view
(`ugr::background_from_indirect`, and the doc comment already says so). Naively that
is `cells × 16` calls to `illuminance_parts`, each firing `rays_per_point × bounces`
paths — a full second grid calculation, sixteen times over. Not acceptable.

Two multiplications, both already precedented in this codebase:

1. **One sphere sample set serves all 16 directions.** Sample the incoming indirect
   field at the point *once* with M directions, keep `(ω_i, L_i)`, then
   `E_v(φ) = Σ L_i·max(0, ω_i·n_φ)·w_i` for every φ from the same samples. New
   method on the evaluator, next to `scalar` (`calc.rs:146`), which already walks a
   deterministic Fibonacci sphere for exactly this kind of reuse:

   ```rust
   /// Indirect vertical illuminance at `point` for `n` horizontal azimuths,
   /// from ONE shared set of hemisphere samples.
   pub fn indirect_vertical_rose(&self, point: Vec3, n: u32) -> Vec<f64>;
   ```

2. **The background field is smooth; the glare field is not.** Evaluate the rose on a
   **coarse sub-grid** — 12 × 12, the resolution `cylindrical_avg` already uses
   (`light.rs:1006`, with the same reasoning written out) — and bilinearly
   interpolate to each observer cell. The sharp, position-sensitive part is the glare
   sum, and that stays exact at full resolution.

Net: **the UGR map costs about one extra coarse pass**, on the order of the
cylindrical figure already computed per room. It fits inside the existing
`CalcProgress` step structure (`light.rs:1005`) without changing the shape of a run.

Determinism is preserved for free — `Evaluator::rng_at` (`calc.rs:70`) is seeded from
the point, so re-running gives the same map. That note in the source (*"a designer
who re-runs a calculation and gets 497 lx instead of 499 cannot tell a change they
made from sampling noise"*) applies with more force to a coloured map than to a
number.

---

## 6. Validation — and the test that settles the argument

### 6.1 The acceptance test: reproduce the datasheet in the datasheet's own room

Build the **CIE 117 standard room in code** — the rectangle in multiples of the
height above eye level, reflectances 0.70 / 0.50 / 0.20, the stated SHR, the observer
at the wall mid-point at 1.2 m looking crosswise and endwise — populate it with
`FONDO.ldt`, and check the **direct calculation reproduces the tabulated UGR_L within
about ±1 unit**. The DIALux reports quote 15.

**This is the whole political argument, as an assertion.** Once the engine reproduces
the table value *in the table's room*, every later "your room is 22" is credible,
because the same code produced both. Without it, a client with a UGR<19 datasheet and
a red map has no reason to believe the map.

Put it in `cad_light/tests/` beside `identical_dialux.rs`, which already frames the
distinction correctly (its `ugr_from_the_seated_observer` comment is the reasoning
this test turns into a check). It **replaces** that test's `println!` with an
assertion — the reason it prints today is that the standard room had never been built.

### 6.2 Properties that must hold whatever the convention

Extend the existing set in `ugr.rs`, which already pins four (off-axis damping,
background, aperture, accumulation). Add:

* **Aim matters** — the regression for §2.1. A fitting tilted to point *at* the
  observer rates worse than the same fitting pointing at the floor. This test fails
  on today's code, which is the point of writing it.
* **Untilted is bit-identical** to the pre-refactor path — the safety property for
  §2.1 and §3.
* **Turning away helps** — `min ≤ mean ≤ max` per cell, by construction.
* **Occlusion helps** — a wall between eye and fitting removes it from the sum.
* **Height matters** — the standing map differs from the seated one in a room with
  ceiling fittings, and the difference has the sign geometry predicts.
* **Continuity** — no cell differs from its neighbour by more than a few UGR unless a
  source crosses the horizon or an occluder edge. A map that speckles is a map with a
  ray-epsilon bug, and a speckled map is the most obvious way to lose a client's
  confidence in an otherwise correct calculation.

### 6.3 Cross-check

Run the three `tests/Identical testing` scenes at both heights and compare the map's
**wall mid-point, looking in** against DIALux's table figure. They should *not* be
equal — but they should be in the same neighbourhood for those near-standard rooms,
and a gap of 6+ units means a bug, not a methodology difference.

---

## 7. Phases

**Phase 0 — correctness (do not skip; nothing below is trustworthy without it).**
`ugr.rs` §2.1–2.5 plus `STANDING_EYE_M` §2.6, plus the §3 split. Tests: aim,
untilted bit-identity, occlusion, multiplier, `output_scale`. No UI. This is
shippable on its own as a fix to a metric nobody is yet consuming — the cheapest
moment it will ever be to change it.

**Phase 1 — a map, one height, one reduction.** New `cad_light/src/ugr_map.rs`:
`UgrGrid`, `ugr_map_on(ev, plane, opts)`, the `max` reduction, `CellStatus`.
`Evaluator::indirect_vertical_rose` plus `Evaluator::occluded` exposed (`scene` is
private today). Seated 1.2 m only. Plan overlay with EN bands and a hatched unrated
state. **Bump `CALC_EPOCH`** (`light.rs:759`) — the engine now means more by a scene,
and a cached result must not restore as valid.

**Phase 2 — the rose, and the answer to *which way*.** Store all 16 azimuths;
`min` / `mean` / `at(φ)` / `worst_azimuth`; the direction selector; the arrow layer
(§4.3); the **probe pin** — click a point, get the polar plot plus the ranked source
table straight out of `UgrResult::sources` (σ, L, ω, p, term) and the background used.
The probe is the feature that ends the argument in the meeting: it names the three
fittings doing the damage, from that seat.

**Phase 3 — standing layer, report, domain honesty.** The 1.6 m layer; a report page
per room per height (`report/layout.rs`, beside the false-colour page); persistence
in `light_store.rs` and `RoomResult`; §4.6 small-source and large-source branches and
the domain footnote; the §6.1 standard-room test in CI.

**Phase 4 — 3D.** Paint the map on the eye-height plane in `light3d.rs`, and a
**first-person glare view**: put the camera at the observer, render what they see,
and mark each source with its σ / L / p. That is the picture that makes "UGR is about
where you are and where you look" self-evident without a word of explanation.

---

## 8. Open questions — mentor to author, decide before Phase 1

1. **Default map = `max` over azimuth?** Recommended: yes, it is the compliance
   reading and the conservative one. But it will look alarming in rooms that pass in
   practice because nobody faces that way. The mitigation is the §4.3 arrows and the
   `at(φ)` map beside it, not a softer default.
2. **Grid spacing for the observer map.** Recommend a separate `ugr_cell_size`
   defaulting to **0.5 m**, independent of the work-plane cell size. UGR varies
   smoothly in position; 0.2 m cells cost 6× for no readable detail.
3. **Do we quote a single room UGR number?** EN 12464-1 wants one figure per area.
   Recommend **the maximum over the room's rated cells at 1.2 m**, stated as
   *"UGR ≤ n across the task area"*, with the map as the evidence — and never a room
   average, which hides the one seat that fails.
4. **Maintained or initial?** Recommend **maintained**, consistent with every other
   figure SIMLUX prints (§2.5), stated in the footnote.
5. **`STANDING_EYE_M` 1.5 → 1.6 m?** Needs your copy of EN 12464-1. It is a
   two-character change now and a re-issued report later.
