# Recording tones

A "recording tone" is a DSP profile applied to the input signal **before** it hits the WAV writer. It bakes a small chain of filters into every take. The default chain is:

1. **Input gain** (dB) — a simple linear trim, applied first.
2. **High-pass filter** — Butterworth biquad, configurable cutoff.
3. **Noise gate** — peak envelope follower with attack/release smoothing.
4. **Compressor** — feedforward, peak-detected, with makeup gain.

Each block is independently enabled/disabled per profile. There's no parametric EQ or de-esser yet — those are slated for the Clean tab work.

## Built-in profiles

| Preset | HPF | Gate | Compressor | Use case |
|---|---|---|---|---|
| **Guitar** (default) | 60 Hz | off | 2.5:1, 20 / 150 ms, +3 dB makeup | Acoustic or lightly-overdriven electric. Keeps decay; light glue. |
| **Vocals** | 100 Hz | −42 dB, 3 / 80 ms | 3.5:1, 8 / 120 ms, +4 dB | Spoken or sung vocals. Aggressive low cut, gate for breath, intelligibility-first compression. |
| **Wind / Brass** | 50 Hz | off | 2:1, 15 / 180 ms, +1 dB | Sax, flute, trumpet, harmonica. No gate (breath is the sound). |
| **Drums / Percussion** | off | off | 4:1, 3 / 80 ms, +2 dB | Room mic or overheads. HPF off so kick sub-bass survives. |
| **Raw / Clean** | off | off | off | No processing — bit-exact capture. The "I don't trust your defaults" preset. |

The profile is **frozen at the moment recording starts**. You can switch profiles between takes; you cannot switch mid-take.

## Stereo behaviour

When recording in stereo mode, profiles apply through `FilterChainStereo`:

- HPF runs **independently** per channel.
- Gate and compressor envelope detection use `max(|L|, |R|)` and apply identical gain to both sides. This preserves the stereo image — a gate duck or compressor squish never collapses one side while keeping the other open.

## Per-track snapshot

Every `Track` in your project file carries the full parameter set used for its take, in the `profile` field. This means:

- Reopening a project a year later, you can see exactly what was active.
- Sharing a project file ships all its DSP state — no "missing preset" problems.
- Future Clean-tab work can dispatch differently per track based on what was used during capture.

See *Editing profiles (Admin)* for changing the parameters or adding your own.
