# TBSS-FR-0004 — Console mixer + volume automation

| Field | Value |
|---|---|
| **Request ID** | TBSS-FR-0004 |
| **Title** | Hardware-style console mixer on the Mix tab + per-track volume automation (recorded fader gestures replayed via Catmull-Rom splines) |
| **Status** | Proposed |
| **Date filed** | 2026-04-26 |
| **Author** | Claude (authoring assistant) on behalf of project owner |
| **Session serial** | `CLAUDE-2026-04-26-MIXER-RFC-0004` |
| **Depends on** | TBSS-FR-0002 (Mix tab + player, ✅ landed v0.2.0). Composes well with FR-0003 (Import normalization) but doesn't require it. |
| **Breaking changes** | None to users. `Project` schema gains optional automation fields (`#[serde(default)]`); old manifests load unchanged. |

---

## 1. Executive summary

Two interlocking features that share a UI surface — the lower half of the Mix tab — and ship as one coherent RFC because separating them creates UX gaps:

**A · Console mixer** — repurpose the unused area below the multitrack lanes as a hardware-style mixer console. Each track gets a vertical fader strip with name / mute / solo / gain knob / fader / level meter. A master strip on the far right shows bus level + master gain. Looks and feels like the bottom half of an SSL or Yamaha console.

**B · Volume automation** — per-track and master fader movements can be **armed and recorded** during playback. A subsequent play replays the recorded gestures by interpolating between timestamped points using Catmull-Rom splines (`splines` crate) — natural-feeling reproduction of the user's hand movements, the way a studio console with motorised faders does it.

The two features compose: the mixer gives the user faders to move; the automation system records and replays the moves. Neither makes much sense on its own.

## 2. Problem

The Mix tab today (v0.2.0+) shows multitrack waveform lanes with per-track controls in a 280-px header column: name, mute, A/B bypass, gain (drag value), Correction button. The bottom of the tab is empty whitespace below the lanes.

Two missing capabilities:

1. **Mixing ergonomics.** A drag-value gain control is fine for set-and-forget. It's terrible for "ride the vocal", "pull the bass back during the bridge", "fade out the master over four bars". Users want a fader they can grab while the song plays.

2. **Capturing mix moves.** Once you've ridden a fader to the right shape, the next play loses the gesture — no automation system. Tracking the moves over time is a studio-console fundamental TinyBooth doesn't yet honour.

## 3. Proposal — Console mixer (Part A)

### 3.1 Layout

The Mix tab splits vertically:

```
┌─────────────────── Mix ───────────────────────────────┐
│ Transport bar (▶ ⏸ ⏹  time  rate)                     │
├───────────────────────────────────────────────────────┤
│ Multitrack lanes (timeline + waveforms + playhead)    │
│   ~60 % of remaining height, scrollable               │
├───────────────────────────────────────────────────────┤
│ Console deck (~40 % of remaining height, scrollable   │
│ horizontally if track count exceeds available width)  │
│                                                       │
│ ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐         ┌────┐    │
│ │Voc │ │Drum│ │Bass│ │Keys│ │ FX │   ...   │MAST│    │
│ │M S │ │M S │ │M S │ │M S │ │M S │         │ R  │    │
│ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │         │ ▮  │    │
│ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │         │ ▮  │    │
│ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │ │ ▮  │         │ ▮  │    │
│ │ ║▓ │ │║▓  │ │║▓  │ │║▓  │ │║▓  │         │║▓▓ │    │
│ │ ║░ │ │║▓  │ │║░  │ │║▓  │ │║░  │         │║▓░ │    │
│ │-3.0│ │ 0.0│ │-1.5│ │+2.0│ │-6.0│         │-4.5│    │
│ └────┘ └────┘ └────┘ └────┘ └────┘         └────┘    │
└───────────────────────────────────────────────────────┘
```

The split is resizable via a draggable horizontal bar between lanes and console — same `egui::SidePanel`/`TopBottomPanel` pattern we already use elsewhere.

### 3.2 Per-track strip

Top to bottom:

| Element | Notes |
|---|---|
| Track name | Truncated with ellipsis to fit strip width (~70 px). |
| `M` mute toggle | Same flag as the lane-header mute. Synced. |
| `S` solo toggle | **New**. When any solo is active, every non-soloed track is muted at the player level. |
| `R` arm-automation toggle | Part B (§4). Greyed out until automation lands. |
| Vertical fader | Tall slider, range −∞ (`−60 dB` floor) to `+6 dB`. Click-drag to move, scroll-wheel for fine. Reads & writes `track.gain_db`. |
| Numeric dB readout | Below the fader, three significant digits. |
| Peak meter | Vertical bar adjacent to the fader. Driven by a new per-track post-fader-post-correction peak atomic published from the audio thread (cheap — single `AtomicU32` per track). Decay envelope identical to the existing input meter on the Record tab. |

The lane-header gain `DragValue` from v0.2.0 is removed — replaced by the strip fader. Lane-header keeps mute / A-B bypass / Correction button (no duplication of fader UI).

### 3.3 Master strip

Mirror of a track strip but:

- Name: `MASTER`
- `M` / `S` / `R` (mute / solo / arm) — only `R` is meaningful. Mute makes no sense at the bus; solo is per-track.
- Fader — applies to the post-mix bus. New `Project.master_gain_db: f32` field, `#[serde(default)] = 0.0`.
- Meter — sums `(L_post, R_post)` from the audio thread; shows two thin bars side-by-side for L and R.

### 3.4 Solo semantics

Standard "AFL" (after-fader listen) solo:

- If any track has `solo = true`, every track without `solo = true` is **playback-muted** for the duration.
- `mute` and `solo` are independent flags — a track can be muted-and-soloed (rare but valid).
- Solo state lives in the player's `TrackPlay` (new `AtomicBool` alongside `mute`). Not persisted across project saves — solo is a transient listening tool, like A/B bypass.

## 4. Proposal — Volume automation (Part B)

### 4.1 Data model

New optional struct on each Track:

```rust
#[serde(default)]
pub gain_automation: Option<AutomationLane>,
```

And an analogous one on Project for the master:

```rust
#[serde(default)]
pub master_gain_automation: Option<AutomationLane>,
```

Where:

```rust
pub struct AutomationLane {
    pub points: Vec<AutomationPoint>,
}

pub struct AutomationPoint {
    pub time_secs: f32,
    pub gain_db: f32,
}
```

Sorted by `time_secs` (invariant; the recorder enforces it). Empty `points` is equivalent to "no automation" — but we keep the lane present to mark that the track has been armed at least once (UI signal: a thin curve drawn over the fader's groove showing where the recorded curve sits).

### 4.2 Recording

Per-strip `R` (arm) toggle. When at least one strip is armed and playback is in `Playing` state, a recorder thread (or a slim ring buffer flushed by the audio thread; see §4.5) samples the current `track.gain_db` (whatever the user is dragging the fader toward) at ~60 Hz and pushes a point onto a scratch lane:

- Push only if the new value differs from the last by more than ~0.05 dB (decimation — humans don't drag faders at sample rate).
- On Stop, the scratch lane replaces the track's `gain_automation` field. Project marked dirty.
- Re-arming and re-recording over an existing lane overwrites it. (Punch-in / partial overwrite — Phase-3 polish.)

The fader is **still draggable** while armed-and-playing — that's the whole point. The user is performing.

### 4.3 Playback

When a track has a non-empty `gain_automation` and recording is **not** armed, playback walks the lane. At each frame:

1. Look up the two bracketing points for current `position_secs`.
2. Interpolate using **Catmull-Rom splines** (smooth, natural for human gestures — see §4.4).
3. Use the interpolated value as the effective gain instead of the static `track.gain_db`.

The fader **visualizes the interpolated value** during playback — it moves on its own, exactly like a motorised studio fader. The user can grab it to override; if they're not armed, the override is a transient touch and the lane resumes when they let go (called "ride and release" in console parlance).

### 4.4 Why Catmull-Rom — and the cool-crate pick

A linear interpolation between recorded points sounds fine on faders that move slowly but introduces audible kinks at every captured timestamp on faster gestures — the gain trajectory has C0 continuity but not C1. Real motorised faders move with a smooth velocity profile; reproducing that is what the curve choice buys.

**Recommended crate: [`splines`](https://crates.io/crates/splines)** (MIT, ~250k downloads, mature).

```rust
use splines::{Interpolation, Key, Spline};

let keys: Vec<Key<f32, f32>> = lane.points.iter()
    .map(|p| Key::new(p.time_secs, p.gain_db, Interpolation::CatmullRom))
    .collect();
let spline = Spline::from_vec(keys);
let gain_db = spline.sample(position_secs).unwrap_or(default_gain);
```

Catmull-Rom requires four keys to interpolate (one before, two bracketing, one after). The `splines` crate handles the boundary cases automatically — outside the interpolable region it returns `None`, in which case we fall back to the static `track.gain_db`.

**Alternatives considered:**

| Crate | Verdict |
|---|---|
| [`splines`](https://crates.io/crates/splines) | ✅ Picked. Catmull-Rom built in; `Spline::sample` is exactly the API we need; mature; pure Rust; MIT. |
| [`uniform-cubic-splines`](https://crates.io/crates/uniform-cubic-splines) | Capable but oriented at uniformly-knot-spaced curves; our timestamps are non-uniform by definition (humans don't move faders on a metronome). Workable but less idiomatic. |
| [`minterpolate`](https://docs.rs/minterpolate/) | Provides a free `catmull_rom_spline_interpolate` function; lighter than `splines` but no `Spline` type to hold the keys, so we'd write the framing ourselves. Acceptable second choice if `splines` feels heavyweight. |
| [`keyframe`](https://crates.io/crates/keyframe) | Animation-tweening focused (ease-in / ease-out / cubic Bezier). Wrong shape — we want curve fitting, not ease curves. Skip. |
| Hand-rolled Catmull-Rom (~30 LOC) | Tempting but reinventing the wheel; `splines` saves the boundary-condition logic and is well-tested. |

### 4.5 Audio-thread integration

The audio callback already has `position_frames` per output buffer. For each track:

1. If `track.solo` is unset and any global solo is active, output silence for this track.
2. Compute effective gain: if `gain_automation` is `Some` and recording is not armed → spline-interpolate. Otherwise → `track.gain_db` from the existing atomic.
3. Apply gain after correction, before bus sum (current order preserved).

The spline can be pre-built once when the project loads (or when the user re-records), held as `Arc<Spline<f32, f32>>` in `TrackPlay`. The audio thread does only `spline.sample(t)` per frame — no allocations, no locks once the Arc is published.

For master automation: same logic, applied after the bus sum and before the soft-limit.

For the **recorder** path: ~60 Hz sampling on the UI thread is fine — drag events are already at that rate from egui. The UI thread pushes points into a `Vec<AutomationPoint>` directly (UI-thread-owned) and on Stop ships the lane back into the project model and rebuilds the spline. No audio-thread involvement; recording is a UI concern.

### 4.6 Visualisation

- **In the lane (timeline)**: optional thin curve drawn under the waveform showing the gain envelope across time. Same coordinate system as the playhead. Useful for seeing where you rode the fader.
- **On the strip (fader)**: when not armed and the playback enters a section with automation, the fader **moves** to follow the interpolated value. When armed, the fader follows the user's mouse and the recorder captures the trajectory.

## 5. Phasing

| Phase | What lands | Lift | Tag |
|---|---|---|---|
| **A · Console mixer** | Vertical strip layout, faders, per-track meters, master strip + `Project.master_gain_db`, solo flag with mute-the-rest semantics. **No automation.** | ~3 days, ~600 LOC | v0.3.0 |
| **B · Volume automation** | `AutomationLane`, recorder, `splines` crate, audio-thread interpolation, per-strip arm toggle, on-strip + on-lane visualisation. | ~4 days, ~500 LOC | v0.3.1 (or 0.4.0 if it lands as a "milestone" alongside other polish) |
| **C · Polish** | Punch-in / partial-overwrite re-record, automation editing (drag points directly in the lane), latch / touch / read modes, copy automation lane between tracks. | tbd | later |

A and B are independently shippable. A delivers a real ergonomic upgrade on its own; B without A makes no sense (you need faders to capture).

## 6. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Vertical-fader widget is non-trivial in egui (it has horizontal sliders out of the box; vertical needs custom drawing) | egui's `Slider` already supports `.vertical()`. Confirmed in upstream docs and used by other audio-tool egui apps. The strip is mostly stock primitives. |
| Per-track meter publishing from the audio thread adds a per-track AtomicU32 store every callback | Trivially cheap. Existing input-meter atomic does the same and never shows up in profiling. |
| Catmull-Rom can over/undershoot between widely-spaced points (classic spline behaviour) | Decimation threshold in §4.2 (push only when ≥0.05 dB delta) keeps point density up where the fader is moving. For long quiet stretches the gap is fine — the spline degenerates into a near-flat line. If overshoot becomes audible we can switch to centripetal Catmull-Rom (parameterised in `splines`) or clamp the output. |
| User confusion: "why is my fader moving on its own?" | First-run hint in the strip's `R` button hover ("Arm to record fader moves; play to replay them"). The Manual chapter §10 will gain an Automation section. |
| Solo + automation interaction edge cases (track is soloed AND has automation that reaches the −60 dB floor at some point) | No interaction — solo gates the bus sum, automation drives the per-track gain. Both apply independently. Document the order. |
| Large automation lanes (10 minutes × 60 Hz × ~16 tracks = ~600k points) | Decimation in §4.2 keeps real lanes well under 10k points per track. If profiling shows the spline build is slow, switch to chunked rebuild on dirty regions only. |

## 7. Open questions

1. Should the master strip show **L+R separately** or a summed "louder of L/R" meter? Studio consoles vary; both are conventional. Default: separate L+R. Cost: a second atomic.
2. Solo behaviour on the master strip — meaningful or no-op? Probably no-op (there's nothing to solo *against* at the bus). Master `S` button greyed.
3. Automation curve drawn **above** or **below** the waveform on the lane? Above is more visible but covers signal; below is cleaner. Default: below, semi-transparent.
4. Should A/B bypass also bypass automation on the affected track? Yes — A/B should mean "raw source as Suno gave it", which means no automation playback. Add to the §4.5 audio-thread flow.
5. Persisting `master_gain_db` and master automation: where? Adding to `Project` directly. Versioning is `#[serde(default)]` so old projects open without it.

## 8. Success criteria

- Console deck visible at the bottom of the Mix tab, faders driven by the same `gain_db` as the lane-header drag value (one source of truth).
- Solo on any track silences all others without affecting the soloed tracks' levels.
- Master fader moves the bus output up/down without re-encoding anything; reset to 0 dB returns to current bus level.
- Per-track meters move during playback and respond visibly to fader rides within ~30 ms.
- Arm a track, play, ride the vocal up and back down over a 10-second section, stop. Re-arm OFF, play again — the vocal rides up and down the same way, with smooth motion (no audible kinks at point boundaries).
- Open a project saved on v0.2.x in this version: existing `gain_db` values are preserved; missing automation fields default to None; everything works.

## 9. Out of scope

- Pan automation (and pan in general — TinyBooth is currently centre-only-or-true-stereo). A pan strip element would need pan to exist first. Separate FR.
- Bus / aux / send routing. The single-bus master is intentional for v0.3 scope.
- VST / external plugin hosting. Out of scope for the entire project.
- Per-correction-parameter automation (record EQ band sweeps, comp threshold rides, etc.). Conceivable later — the AutomationLane abstraction generalises if we ever want it — but not in v0.3.

---

*Session serial `CLAUDE-2026-04-26-MIXER-RFC-0004`. Quote when requesting revisions that should build on this design.*

**Sources for crate research:**
- [splines on crates.io](https://crates.io/crates/splines)
- [splines::interpolation::Interpolation docs](https://docs.rs/splines/latest/splines/interpolation/enum.Interpolation.html)
- [uniform-cubic-splines on crates.io](https://crates.io/crates/uniform-cubic-splines)
- [minterpolate::catmull_rom_spline_interpolate docs](https://docs.rs/minterpolate/latest/minterpolate/fn.catmull_rom_spline_interpolate.html)
- [Centripetal Catmull-Rom — Wikipedia](https://en.wikipedia.org/wiki/Centripetal_Catmull%E2%80%93Rom_spline)
