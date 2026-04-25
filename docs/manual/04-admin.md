# Editing profiles (Admin)

Open via **Admin → Recording-tone profiles…** in the top menu, or click the **Admin…** button next to the Recording-tone dropdown on the Record tab.

The Admin window is a non-modal floating panel — you can edit profiles while a recording is in progress. The change applies to the **next** take, never the current one.

## Layout

- **Left panel** — list of profiles. The active profile is marked `●`. Click any name to edit it on the right.
- **Right panel** — editor for the selected profile. Every numeric parameter is a `DragValue` — scrub with the mouse or click to type a value.
- **Top bar** — `+ New` (duplicates the current profile), `Save all` (writes to `profiles.json`), `Reset to defaults` (discards all custom edits).

## Fields explained

### Identity
- **Name** — what shows up in the Recording-tone dropdown. Must be unique.
- **Description** — shown as a tooltip when hovering the dropdown entry.

### Input
- **Input gain (dB)** — flat trim before any other processing. Negative attenuates a hot mic; positive pushes a quiet one. Range −24 to +24.

### High-pass filter
- **Enabled** — bypass switch.
- **Cutoff (Hz)** — where the rolloff starts. Common values: 60 for guitar, 100 for vocals, 30 for full-range, off for drums. Range 20–1000.

### Noise gate
- **Enabled** — bypass switch.
- **Threshold (dB)** — signal below this gets muted. −60 is loose, −40 is tight, −30+ aggressively chops.
- **Attack (ms)** — how fast the gate opens when signal exceeds threshold. 1–5 ms feels instant.
- **Release (ms)** — how fast it closes when signal drops below. Too short and you hear the gate "chatter"; too long and breath leaks through.

### Compressor
- **Enabled** — bypass switch.
- **Threshold (dB)** — level above which compression kicks in.
- **Ratio (x:1)** — how aggressively the compressor pulls down signal above threshold. 2:1 is gentle; 4:1+ is broadcast-style.
- **Attack (ms)** — how fast compression engages. Shorter catches transients; longer lets them through.
- **Release (ms)** — how fast it disengages. Should roughly match the rhythm of the source — slow for vocals, fast for drums.
- **Makeup gain (dB)** — flat boost after compression to restore perceived loudness.

## Save semantics

Edits are **in-memory until you click Save all**. If you make changes and quit without saving, they're gone. The profiles file is a plain JSON document — you can also edit it externally with any text editor; the path is shown at the top right of the window.

## Adding profiles

`+ New` clones the currently selected profile with `(copy)` appended to its name. Rename it, tweak parameters, save. The new profile appears in the Record tab dropdown immediately.

## Resetting

`Reset to defaults` replaces the entire profiles list with the built-in five (Guitar / Vocals / Wind / Drums / Raw). It then writes that list to disk. There is no undo — back up `profiles.json` first if you have custom presets you want to keep.
