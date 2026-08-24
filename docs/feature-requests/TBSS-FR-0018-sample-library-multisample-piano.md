# TBSS-FR-0018 — Tracker sample library: free online instruments, multisample zones, five-octave piano editor

| | |
|---|---|
| **ID** | TBSS-FR-0018 |
| **Title** | In-app acquisition of free instrument samples (to spec, at volume) + multisample instruments edited through a five-octave piano with wave-editor lanes |
| **Status** | 📝 Proposed |
| **Filed** | 2026-08-24 |
| **Requested by** | Carlos ("get free online instrument samples for tracker, to spec, research should yield large volumes. each instrument can be represented as a five octave piano that spawns wave editor tracks like the ones in mixer/record") |
| **Depends on** | FR-0014 (tracker engine + instrument model), FR-0015 (wave-editor affordances: scrub playhead + drag selection), `audiodecode`, `zip` dep (already present for Suno bundles) |

## Executive summary

Two halves that meet in the middle. **Acquisition**: a Sample Library
browser that downloads *curated, license-vetted* instrument packs from
free online sources — thousands of per-note recordings per source — and
auto-builds tracker instruments from them by parsing pitch out of the
filenames ("to spec": every sample lands with a known root note).
**Representation**: instruments become **multisample** (per-note zones,
not one stretched sample), edited through a **five-octave piano** —
keys show zone coverage, clicking auditions the note, and the selected
key spawns a **wave-editor lane** with the exact affordances the
Record/Mix surfaces already have (scrubbable playhead, drag selection) —
where the selection *is* the zone's trim/loop window.

## Research: the sources (verified 2026-08-24)

| Source | Volume | Format / pitch-in-filename | License | Acquisition |
|---|---|---|---|---|
| **Philharmonia Orchestra** ([sound samples](https://philharmonia.co.uk/resources/sound-samples/)) | thousands of samples, all orchestral instruments, multiple dynamics/articulations per note | mp3; note encoded in filename (`cello_A2_1_forte_arco-normal.mp3` pattern) | Free for any use incl. commercial; **must not be redistributed "as is"** → we must download-on-demand to the user's machine, never bundle | per-instrument zips |
| **VCSL — Versilian Community Sample Library** ([github.com/sgossner/VCSL](https://github.com/sgossner/VCSL)) | multi-GB; aerophones/chordophones/idiophones/membranophones + TX81Z; 2–3 velocity layers, 20–75 MB per instrument | wav 44.1/48k 16/24-bit; documented naming syntax with pitch | **CC0** — bundle-able, no strings | release zips |
| **University of Iowa MIS** ([listed in the classic roundup](https://www.metafilter.com/102076/Free-HighQuality-Musical-Instrument-Samples)) | full orchestral coverage, chromatic per-note recordings | aiff; note in filename (`Piano.ff.C4.aiff` pattern) | free for any use | per-instrument pages |
| **Virtual Playing Orchestra** ([virtualplaying.com](https://virtualplaying.com/virtual-playing-orchestra/)) | curated best-of blend of Sonatina/No-Budget/VSCO2-CE/Iowa/Philharmonia | sfz + wav | free | zips |
| Freesound API | effectively unbounded | mixed CC licenses per file | per-file | **deferred** — OAuth per-download + per-file license vetting is real friction; the curated packs above already satisfy "large volumes" |

Licensing rule baked into the design: **CC0 sources** may be listed with
direct download URLs and even mirrored; **Philharmonia-class sources**
("free to use, don't redistribute as samples") are downloaded by the
*user's* machine from the *source's* URL on demand — TinyBooth ships
only the manifest (name, URL, parse rules, license text shown in the
UI). Nothing sample-shaped ever enters the repo or installer.

## Proposal

### E1 — pack manifest + downloader

- `docs/../assets/sample-packs.json` (shipped, curated): per pack —
  `name, instrument, source, license { id, summary, url }, download_url,
  archive_kind, parse: { note_regex, octave_convention } `.
- **Sample Library window** (from the Tracker instrument rail): pack
  list with license shown; Download → background thread fetches the zip
  (reqwest, already a dep) into
  `%APPDATA%/TinyBooth Sound Studio/sample-library/<pack>/`, unzips
  (existing `zip` dep), progress + cancel. Zip-slip guarded like the
  Suno importer.

### E2 — pitch-from-filename parser (pure, tested)

One parser, per-source regex table, normalising the three real
conventions found in research: `_A2_`/`_Cs4_` (Philharmonia),
`.C4.`/`.Db5.` (Iowa), `_vl#_rr#` + note token (VCSL). Emits
`(root: Note, velocity_layer, articulation)` per file; unparseable
files are listed, not silently dropped (audit rule: no silent caps).

### E3 — multisample zones in the engine

```rust
struct SampleZone { root: Note, sample_idx: usize } // sorted by root
// TrackerInstrument gains: zones: Vec<SampleZone>
// per-instrument sample storage becomes Vec<DecodedSample>
```

Trigger picks the zone with the **nearest root** to the played note and
pitches relative to *that* root — a C-4 zone never has to stretch to
C-7 if a C-6 zone exists. Back-compat: a single-sample instrument is one
zone at `base_note` (serde default), so every existing song loads
unchanged. Engine tests: nearest-zone selection, zone-boundary
correctness, single-zone equivalence with today's behavior.

### E4 — the five-octave piano

In the instrument editor: a 60-key piano (C-2…B-6, the tracker's
practical playing range; scrollable to the full 0–119 later).

- **Coverage display**: keys with a zone root are accented; keys covered
  only by pitch-stretch are dimmer; the nearest-zone assignment is
  visible at a glance.
- **Click = audition**: renders that single note through the real engine
  (filter, gain, loop — what playback will do) into a preview session,
  under the app-wide exclusivity rule.
- **Keyboard**: the piano participates in the FR-0016 editor scope with
  the same FT2 key map as the grid — press keys to audition without
  entering notes.

### E5 — wave-editor lanes per key ("like the ones in mixer/record")

Selecting a key opens the backing zone's waveform as a lane with the
established language:

- peaks rendering + **scrubbable amber playhead** while auditioning
  (FR-0015's exact interaction),
- **drag-selection on the waveform sets the zone's trim window**, and
  in loop modes a second handle pair sets `loop_start/loop_end` —
  turning what is today a pair of raw `u64` fields into a visual edit,
- the selection tools write back to the instrument (song dirty →
  re-render at the loop boundary, as the tracker already does).

## Implementation notes

Epics land in order E1→E5; E2 and E3 are pure with tests before any UI.
Downloads and unzips run off the UI thread (house rule). The manifest
starts small and curated — Philharmonia strings/brass/woodwind/perc,
VCSL keys/idiophones, Iowa piano — and grows by editing JSON, not code.

## Open questions

1. Velocity layers: VCSL/Philharmonia ship 2–3 dynamics per note. v1
   picks one layer per zone (prefer *forte/mf*)? Or map the volume
   column to layer selection (real tracker instruments do this)?
   (Lean: one layer in v1; volume-column layer switching is a natural
   E6.)
2. Disk budget: packs are 20 MB–multi-GB. Per-pack size shown before
   download + a library-size readout in the window — enough, or do we
   need a cap/eviction story? (Lean: show sizes, let the user manage.)
3. Should ⤓ bake embed the source pack attribution into the exported
   WAV's TBSS metadata chunk (`wav_meta`)? (Lean: yes — provenance is
   already the house pattern.)

## Success criteria

- From a clean install: open Sample Library → download the Philharmonia
  cello pack → an instrument appears with dozens of zones parsed from
  filenames → the piano shows coverage → clicking C-3 auditions a real
  cello C-3 (not a stretched sample) → drag-select trims a zone → the
  pattern plays it → bake lands a stem.
- Zero samples in the repo/installer; every pack's license visible
  before download; unparseable files reported.

Sources: [Philharmonia sound samples](https://philharmonia.co.uk/resources/sound-samples/) · [VCSL (CC0)](https://github.com/sgossner/VCSL) · [Virtual Playing Orchestra](https://virtualplaying.com/virtual-playing-orchestra/) · [classic free-samples roundup incl. Univ. of Iowa MIS](https://www.metafilter.com/102076/Free-HighQuality-Musical-Instrument-Samples)
