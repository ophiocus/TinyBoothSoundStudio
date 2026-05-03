# Cleaning Suno stems with TinyBooth

This is the workflow TinyBooth is built around: Suno bundle → cleanup
→ mix → release. Each step uses the v0.4.0 Suno-aware features (per-role
correction presets, import-time coherence verification, polarity flip,
DC trim, Nyquist cleanup, BS.1770 LUFS metering, project trim) without
asking you to know what BS.1770 means or which knob does what.

If you're new to TinyBooth, start with
[Importing Suno stems](manual/07-suno-import.md) for the basics, then
come back here for the full mixing flow. The
[Mix tab manual](manual/10-mix.md) covers the moment-to-moment console
layout in more detail.

---

## 1. Import the bundle

Suno gives you two delivery shapes — a zipped "Download All" archive
or a folder of individual stem WAVs. TinyBooth handles both via the
**File** menu.

![File menu open showing the four Import Suno options: stems folder, stems zip, mixdown folder, mixdown zip](assets/docs/suno-cleaning/01-import-menu.png)

Pick the matching entry. The dialog asks you where to extract the
project; the default is a sibling folder next to the source.

---

## 2. Read the import report

When the import finishes, a modal pops up with everything TinyBooth
learned about the bundle:

- **Stem count** and where each one landed.
- **Mixdown LUFS** — the bundled mixdown's BS.1770-4 integrated
  loudness, computed once at import. This is your *target loudness*
  for the rest of the workflow.
- **Coherence verdict** — sums every stem at unity, subtracts the
  mixdown, reports the residual relative to the mixdown's own RMS:

  | Residual relative to mixdown | Verdict |
  |---|---|
  | ≤ −30 dB | Stems compose cleanly into the mixdown |
  | ≤ −20 dB | Stems mostly compose — minor residual |
  | ≤ −10 dB | Noticeable residual — a stem may be missing or anti-phase |
  | > −10 dB | Stems do NOT compose into the mixdown |

- **Per-stem polarity check** — Pearson correlation between each
  stem and the mixdown. Stems with `r < −0.3` get an `⚠ ANTI-PHASE`
  badge. Don't panic — these are easy to fix in step 4.

![Import-result modal showing the kept-stems list, mixdown LUFS line, and a coherence block with the verdict and per-stem correlation values](assets/docs/suno-cleaning/02-import-report.png)

If anything in the report looks wrong (a stem flagged anti-phase, a
weak coherence verdict), make a mental note. Then close the modal.

---

## 3. Open the Mix tab

Switch to the **Mix** tab. The bundle is already loaded as a
multitrack project with each stem on its own lane.

![Mix tab after a fresh Suno import, showing waveform lanes for each stem on top and channel strips on the bottom, with the per-role labels visible](assets/docs/suno-cleaning/03-mix-tab.png)

Each stem already has a correction chain attached — the appropriate
Suno-X preset for its role:

| Role | Auto-seeded chain |
|---|---|
| Vocals | `Suno-Vocal` — HPF 90 Hz, mud cut, presence + air, de-esser, Nyquist clean |
| Backing Vocals | `Suno-BackingVocal` — tighter compression, lighter de-essing |
| Drums | `Suno-Drums` — no HPF (kick needs sub), box cut, stick attack, DC remove |
| Bass | `Suno-Bass` — HPF 30 Hz, mud scoop, slow-attack comp, DC remove |
| Electric Guitar | `Suno-ElectricGuitar` — low-mid cut, presence lift |
| Acoustic Guitar | `Suno-AcousticGuitar` — body + air, light compression |
| Keys | `Suno-Keys` — mud cut, presence lift |
| Synth / Lead | `Suno-Synth` — AI-shimmer notch at 14 kHz, Nyquist clean at 17 kHz |
| Pads | `Suno-Pads` — cloud cut |
| Percussion | `Suno-Percussion` — snap lift, air shelf |
| FX / Other | `Suno-FxOther` — light glue, conservative |

The defaults are tuned for typical Suno output. You'll tune them
per-track in step 6.

---

## 4. Fix anti-phase stems

If the import report flagged any stem as `⚠ ANTI-PHASE`, click the
**Ø** button on that stem's channel strip. The button highlights
when the polarity flip is on; the audio inverts immediately on the
next playback cycle.

![Close-up of a channel strip showing the Ø polarity-flip button highlighted in active state alongside M/S/R](assets/docs/suno-cleaning/04-polarity-flip.png)

A polarity-flipped stem on its own sounds identical to its
non-flipped counterpart — your ear can't hear absolute polarity. But
when summed with the other stems, an inverted stem cancels rather
than adds. Flipping it brings the cancellation back into alignment.

To verify: the project's `tracks/` folder still contains the
original Suno mixdown (TinyBooth keeps it as a reference rather than
summing it as a track). You can solo your mix and the mixdown
side-by-side in any audio tool to A/B.

---

## 5. Trim, if needed

Suno songs sometimes have a few seconds of dead air at the start, a
fade-out tail at the end, or a count-in you don't want. Open
**Project** tab → **✂ Trim project…**.

![Project tab with the Trim project button highlighted in the top header row](assets/docs/suno-cleaning/05-project-trim-button.png)

The trim panel shows a waveform thumbnail of the bundled mixdown (or
the first track if no mixdown). Enter `mm:ss.mmm` start and end
times — the markers update live as you type, and the panel tells
you exactly how much it'll cut from each end.

![Trim project modal showing the waveform thumbnail with two coloured vertical markers (start and end), the time-entry boxes below, and the Apply button](assets/docs/suno-cleaning/06-trim-panel.png)

Hit **✂ Apply trim**. Every WAV in the project (every stem and the
bundled mixdown) gets cropped to the same range, atomically. The
trim is **destructive** — re-import the bundle if you need to
recover the originals.

Coherence stays valid post-trim because every file shares the same
new frame-0; the bundled mixdown gets the same crop as the stems.

---

## 6. Tune the chains

The auto-seeded Suno-X presets are starting points, not finishing
points. For each stem worth tuning:

1. **Solo** the stem (the `S` button on its strip).
2. Click **+ Correction** on the strip to open its chain editor.
3. Adjust EQ bands, compressor, de-esser, gate, DC trim, Nyquist
   clean. Changes apply on the next playback cycle.
4. **Un-solo** and listen in context.

![Per-track Correction editor floating window showing the Input / High-pass / Suno cleanup / Parametric EQ / De-esser / Noise gate / Compressor sections with their drag-value controls](assets/docs/suno-cleaning/07-correction-editor.png)

A few rules-of-thumb worth keeping:

- **Nyquist clean** is on by default for every Suno-X preset. Turn it
  off only if you specifically want the top-octave shimmer.
- **DC remove** is on for drums / bass / percussion. It's cheap and
  reclaims a few millivolts of headroom; rarely worth turning off.
- The **de-esser** is on for vocals only by default. If a synth lead
  has harsh sibilance-like artefacts, enabling it briefly can help.
- The **compressor makeup** field is *not* an output trim; it's there
  to put back what the compressor takes off. If your mix is too quiet
  after compression, raise makeup, not the master fader.

For the per-tab UI details, see [the Mix tab manual](manual/10-mix.md)
and [the Admin / profile-editor manual](manual/04-admin.md).

---

## 7. Gain-stage to a target

The transport bar shows the master bus's loudness in real time:

![Mix tab transport bar with the LUFS readout visible: M -16.2 / I -14.7 LUFS, next to the sample-rate display](assets/docs/suno-cleaning/08-lufs-readout.png)

- **M** — momentary (mean over the most recent 400 ms).
- **I** — gated integrated (whole-programme mean per BS.1770-4 §5.1
  with the −70 LUFS absolute and −10 LU relative gates applied).

Reads `—` until 400 ms of audio have played. Resets on Stop.

Streaming-service targets:

| Target | LUFS |
|---|---|
| Spotify | −14 |
| Apple Music | −16 |
| Tidal | −14 |
| YouTube Music | −14 |
| Broadcast (EBU R128) | −23 |

Adjust the master fader to bring `I` toward your target. Reference
point: the bundled Suno mixdown's own LUFS was logged at import
time (open the import log file under
`%APPDATA%\TinyBooth Sound Studio\logs\`). Matching that loudness
makes your remix sit at the same level as the original Suno output.

---

## 8. Export

Switch to the **Export** tab. Pick a format and a target file.

![Export tab showing the format dropdown (WAV / FLAC / MP3 / Ogg Vorbis / Ogg Opus / M4A-AAC) and the bitrate selector](assets/docs/suno-cleaning/09-export-tab.png)

Native WAV export is built in. The lossy / FLAC formats route through
`ffmpeg`, which TinyBooth looks for next to the executable or on
your `PATH` (see the
[Export manual](manual/06-export.md) if it isn't found).

The export sums every track through its correction chain at the
master fader's *static* gain (automation lanes are honoured). No
limiter is applied; if your mix peaks above 0 dBFS the export will
clip — keep an eye on the master peak meter while you set the fader.

---

## What's deferred to a later release

The v0.4.0 Suno-cleaning surface is intentionally focused. A few
adjacent features are on the roadmap but not yet shipped:

- **Reference playback A/B** — toggle between your mix and the
  bundled mixdown at matched loudness, single-button. The LUFS meter
  + the import-time mixdown-LUFS reading are the foundation; the
  playback-source swap arrives in v0.5.0.
- **Multi-take browser** — load multiple Suno generations of the
  same song side-by-side. Borrow stems across takes. Multi-take A/B.
- **Per-track non-destructive trims** with drag handles on the Mix
  tab waveform lanes. The current trim panel is project-wide and
  destructive (rewrites WAVs). Per-track non-destructive trims are
  a v0.5.0 sequel.

---

## For contributors

The modules involved in this workflow:

| Module | Role |
|---|---|
| `src/suno_import.rs` | Bundle ingestion (folder + zip), role classification, mixdown detection |
| `src/coherence.rs` | Sum-vs-mixdown residual + Pearson correlation for the polarity check |
| `src/lufs.rs` | BS.1770-4 K-weighting + integrated loudness with gating |
| `src/dsp.rs` | The 11 per-role Suno-X presets + the chain that applies them |
| `src/trim.rs` | Project-level batch trim (`.tmp` + rename atomic writes) |
| `src/ui/mix.rs` | Mix tab — channel strips, waveform lanes, transport bar |
| `src/ui/correction.rs` | Per-track Correction window |
| `src/ui/trim.rs` | Trim panel |

For the cross-cutting view, see
[`docs/architecture.md`](architecture.md).
