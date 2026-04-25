# Recording

The Record tab is the centre of gravity. Everything you capture passes through it.

## Layout

From top to bottom:

1. **Recording tone** dropdown — the active DSP preset (see *Recording tones*). Locked while a take is in progress.
2. **Input device** dropdown + Refresh button.
3. **Source** radio row — Mixdown / per-channel / Stereo.
4. **New track name** field — optional; if blank, takes are auto-named `track-001`, `track-002`, etc.
5. **Transport** — the ⏺/⏹ button, plus elapsed time and the WAV filename when active.
6. **Visualisation** — waveform, spectrum, and peak meter (the layout switches to dual L/R lanes in stereo mode).

## How a take is captured

1. You click ⏺.
2. TinyBooth opens a CPAL input stream on the chosen device.
3. The selected recording-tone profile is **frozen** into a `FilterChain` (mono mode) or `FilterChainStereo` (stereo mode) and runs on the audio thread.
4. Each frame is processed (HPF → gate → compressor → makeup gain) before reaching the WAV writer. **What you record is what plays back.**
5. The take is written to disk as 16-bit PCM WAV under `<project>/tracks/<id>.wav`.
6. A `Track` row is appended to the project and the manifest is saved.

## Why the profile is frozen at take start

Two reasons:

- **No mid-take drift.** Tweaking parameters during recording would create discontinuities in filter state and audible artefacts.
- **Reproducibility.** Each `Track` carries a snapshot of the exact profile parameters used. You can reopen a year-old project, see what chain was active, and rebuild it.

If you want to try different settings, stop, change, record a new take. Cheap.

## Visualisation modes

**Mono recording** — single waveform showing the last 2 seconds of audio, single FFT spectrum, single peak meter.

**Stereo recording** — two stacked waveforms (L on top, R on bottom), one spectrum panel fed by `(L+R)/2` (overlapping stereo spectra are visually noisy without adding information), and a pair of peak meters with their own numeric readouts.

The visualizer animates while recording is active and goes quiet when stopped. Recording continues regardless of which tab you're on or whether the manual is open.

## Where takes go

Each take becomes a separate WAV file in `<project>/tracks/`. The project's `tracks_dir()` is shown at the bottom of the Record tab. If you don't pick a project folder explicitly, TinyBooth creates one in `%APPDATA%\TinyBooth Sound Studio\sessions\session-<timestamp>\`.

To save somewhere else: **File → New project…** picks a folder before recording, or **Project tab → Choose folder…** moves an in-progress project.
