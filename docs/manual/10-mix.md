# Mix tab — multitrack remastering

The Mix tab is the centre of the remastering workflow. Open a project (recorded takes, an imported Suno bundle, or a hand-built combination), play it back through your speakers, dial in per-track corrections, and A/B-compare original vs. processed before committing.

## When the tab is available

The Mix tab is enabled whenever the active project has at least one track. With an empty project it shows a hint pointing you to the Record tab or the Suno import flow.

## Layout

- **Top transport bar** — ▶ Play / ⏸ Pause / ⏹ Stop, plus a `MM:SS / MM:SS` time display showing the current position and the longest track's length, plus a label for the project's sample rate.
- **Track lanes** (one per Track):
  - **Header column (left, ~280 px)**: track name, mute (🔇), A/B bypass, gain (dB drag value), and a "Correction" / "+ Correction" button.
  - **Waveform lane (right, fills the rest)**: pre-computed peak envelope drawn over the whole project's timeline. A short track only fills the leftmost portion of its lane.
- **Synchronized playhead** — a single yellow vertical line crossing every lane at the same X position. Sample-accurate; reads the audio thread's atomic position counter once per UI frame.

## Playback engine

When you first open the Mix tab for a project, TinyBooth pre-loads every track's WAV into memory as 16-bit interleaved samples (cheap memory footprint for typical Suno output: ~140 MB for 12 stems × 3 minutes × 48 kHz stereo). It then opens a CPAL output stream on the system default device at the project's sample rate.

The audio callback mixes every unmuted track into a stereo bus, applying each track's correction chain (when present and not bypassed), gain, and a soft-limit. Mono tracks are centre-panned. Stereo tracks pass through L/R unchanged.

Tracks must share a single sample rate. If they don't, the player errors out — resampling is on the Phase-3 list.

## Per-track controls

### Mute (🔇)
Excludes the track from the live playback mix and from export. The track's WAV file isn't touched.

### A/B bypass
When **on**, the track's correction chain is **skipped during playback** — you hear the unprocessed source. When **off**, the chain runs. Toggle on the fly to compare original vs. corrected without losing your settings.

A/B affects **playback only**. Export always honours the persisted correction; if you want to ship an "uncorrected" mix, disable correction entirely (see below).

### Gain
Drag value in dB, range −24 to +12. Applied at playback and at export mixdown.

### Correction button
- **"+ Correction"** (no chain set) — clicking seeds the track with a clone of the Suno-Clean preset and opens the Correction editor. Tweak from there.
- **"Correction"** (chain set) — opens the editor on the existing chain.

## Correction editor

A floating window opened from the Correction button. Same chain shape as a recording-tone profile (input gain → HPF → 4-band EQ → de-esser → gate → compressor → makeup), edited live.

Every change applies to the next playback cycle. The audio thread polls a generation counter and rebuilds its local filter chain when it sees an increment — cheap, glitch-free, no need to stop and start playback.

The header of the editor shows the track's correction state:

- **Active** — chain is running. A "Disable correction" button removes the chain entirely (sets `track.correction = None`).
- **Disabled** — no chain. An "Enable with Suno-Clean preset" button seeds one and switches to Active.

## What gets persisted

Edits to gain, correction profiles, mute state are persisted to the project's `.tinybooth` manifest when you save (File → Save, or the Save button on the Project tab). The mix dirty bit (●) appears next to the project name when there are unsaved changes.

A/B bypass is **not persisted** — it's a transient listening tool. Closing and reopening the project comes back with bypass off (correction active by default).

## Export from a mixed project

Switch to the Export tab as usual. The mixdown algorithm reads each unmuted track, applies its correction chain (if set), applies gain, and sums into the output bus. Output is stereo if any track is stereo, mono otherwise. Soft-limited to [-1, 1].

This is the same pipeline that's audible during Mix-tab playback, so the rendered file matches what you heard within rounding.

## Performance notes

- Pre-loading 12 stems × 3 minutes is ~140 MB. 12 × 5 minutes is ~230 MB. Document budgets if you're working with much longer tracks.
- The audio callback runs at ~256-frame buffers (typically). Each callback locks no more than once per track and only when its correction profile generation has changed since the last build.
- Repaint runs at ~30 fps while playing so the playhead animates smoothly. When stopped or paused, the UI rests.

## Console deck

Below the multitrack lanes, a hardware-style console occupies the lower portion of the Mix tab. Each track gets a vertical fader strip; the master strip sits on the far right. Drag the horizontal divider between lanes and console to resize.

**Per-strip controls (top to bottom):**

- **Track name** — truncated to fit. Hover for the full name.
- **`M` (Mute)** — same flag as the lane-header mute. Excludes the track from the mix.
- **`S` (Solo)** — when any strip's `S` is on, every non-soloed track is silenced. Solo is transient — not persisted across project reloads.
- **`R` (Arm automation)** — when on and playback is running, the strip's fader gestures are recorded as a timestamped automation lane.
- **Fader** — vertical slider, range −60 dB to +6 dB. Drag freely; scroll for fine control.
- **Peak meter** — green / yellow / red bar adjacent to the fader. Driven by the audio thread post-correction-post-fader.
- **dB readout** — current fader value as text.

**Master strip:**

Same shape as a track strip. Mute / Solo on the master are no-ops (nothing to mute against; nothing to solo). The fader applies to the post-bus-sum signal before the soft-limit. Stereo meter shows L and R independently.

## Volume automation

The Mix tab can record fader gestures during playback and replay them on the next play, the way a studio console with motorised faders does it. Replay uses Catmull-Rom interpolation between captured points (via the [`splines`](https://crates.io/crates/splines) crate) so the motion is smooth — no audible kinks at point boundaries.

**Recording:**

1. Click the `R` button on the strip you want to automate. The strip turns red-tinted.
2. Press ▶ Play.
3. Drag the fader as you ride the section — the recorder samples the fader at ~30 Hz, decimates by ≥0.05 dB delta, and stamps each kept point with the current playback time.
4. Press ⏹ Stop, OR click `R` again to disarm without stopping. Either commits the captured lane to the project's manifest (`Track.gain_automation` for tracks; `Project.master_gain_automation` for the master).

**Playback:**

When a strip has automation and `R` is **off**, playback walks the lane, interpolates between points, and drives the fader on its own. Grab the fader during armed-OFF playback to override momentarily — the automation resumes when you let go (a "ride and release" pattern). The recorded curve is also drawn faintly under the waveform on the lane up top for visual reference.

**Re-recording overwrites** the existing lane. Punch-in / partial overwrite is a Phase-3 polish item.

**A/B bypass and automation:** the per-track `A/B` toggle on the lane header bypasses both the correction chain *and* automation when on (so A/B always means "raw source as Suno gave it"). This is the cleanest comparison pair.

**Export:** the rendered file applies every track's correction + per-frame automation gain + master automation, in the same order as Mix-tab playback. What you heard is what you ship.

## Limits (for now)

- No click-to-seek on the waveform lanes (Phase 3).
- No loop region.
- No master limiter on the bus output beyond the soft-limit.
- No resampling — every track must match the project sample rate.
- No per-stem correction-preset library (save/load a chain by name) — Phase 3.
- Automation is volume-only; per-EQ-band / per-correction-parameter automation is not yet supported.
- Re-recording an existing lane overwrites the whole lane — no punch-in.
