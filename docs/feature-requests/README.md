# Feature Requests / RFCs

Design proposals for upcoming TinyBooth Sound Studio features. Each RFC follows a consistent shape: header table, Executive summary, Problem, Proposal, Implementation notes, Risks, Open questions, Success criteria. Quote the **Session serial** when asking for revisions to keep continuity with the underlying research.

| ID | Title | Status | Landed in | Notes |
|---|---|---|---|---|
| **TBSS-FR-0001** | [Suno cleanup mode](TBSS-FR-0001-suno-cleanup.md) | ✅ Implemented (DSP) | v0.1.6 | Parametric EQ + de-esser added to FilterChain(Stereo); Suno-Clean preset shipped. Mix-tab path delivered by FR-0002. |
| **TBSS-FR-0002** | [Multitrack remastering](TBSS-FR-0002-multitrack-remastering.md) | ✅ Implemented | v0.2.0 | Mix tab + cpal output player + per-track corrections + A/B bypass + correction-aware export. |
| **TBSS-FR-0003** | [Import normalization](TBSS-FR-0003-import-normalization.md) | 📝 Proposed | — | LUFS-balance imported stems on ingest; non-destructive (gain only). |
| **TBSS-FR-0004** | [Console mixer + volume automation](TBSS-FR-0004-console-mixer-automation.md) | ✅ Implemented | v0.4.x | Hardware-style fader strips on the Mix tab + recordable Catmull-Rom fader automation. |
| **TBSS-FR-0005** | [Track telemetry](TBSS-FR-0005-track-telemetry.md) | ✅ Implemented | v0.4.13–0.4.37 | Pure-DSP analysis (onsets, YIN pitch, key, drum classes, cross-band coherence) + Coherence Restoration filter. |
| **TBSS-FR-0006** | [The machine-learning boundary](TBSS-FR-0006-ml-boundary.md) | ✅ Accepted (policy) | — | Standing policy: no ML in the core analyzer; ML only as a quarantined opt-in sidecar. |
| **TBSS-FR-0007** | [The `.tib` container — single-file SQLite projects with stem revision history](TBSS-FR-0007-tib-container-revisions.md) | ✅ Landed (MVP) | v0.4.42 | Live `.tib` SQLite format: open/play/trim (reversible, FIFO-5)/export/save/hot-swap/telemetry/import-lands-in-.tib + migrate-on-open prompt. Deferred past MVP: stem/revision browser UI + rollback gesture, recording-into-.tib, .tib→folder Save-As. |
| **TBSS-FR-0008** | [Record tab — recordings browser (full directory listing, waveforms, region export) + repeat-take fix](TBSS-FR-0008-record-tab-recordings-browser.md) | 🔧 In progress | — | Items 1/2/4 fully landed on `main` (unreleased): Loose WAVs section, path-label 📋/📂 buttons, per-take waveform thumbnail + click-drag region selection + 💾 Export Selection (lossless crop via `trim::crop_wav_bytes`). Item 3 partial — orphan-WAV cleanup landed; the underlying cpal repeat-take race needs a real repro signal to fix at root. |
| **TBSS-FR-0009** | [Generator track — binaural / isochronic / layered focus music as a synthesized stem](TBSS-FR-0009-generator-track.md) | 📝 Proposed | — | New `TrackSource::Generator { mode, … }` variant with `Track.locked`; on-demand bake into the existing audio path (`.tib` revision or folder WAV) plus a timestamped export under `exports/generator-bakes/`. Modes: Binaural, Isochronic, Layered (deferred). Dirty-on-param/master/duration change; bake-confirm modal. One open product question — Reading A vs B for "meld with master chain" — flagged in the RFC. |

## Convention

- File name: `TBSS-FR-NNNN-kebab-case-title.md`. Numbers monotonic, never reused.
- Status values: `📝 Proposed`, `🔧 In progress`, `✅ Implemented`, `⛔ Withdrawn`.
- Once an RFC is implemented, the doc stays in this folder as the historical record. The header table's "Status" line is updated; the body is left as-was so the design rationale at decision time is preserved.
- A revision (e.g. FR-0001's §7 was rewritten when Suno's server-side stems made local separation obsolete) is annotated inline rather than splitting into a new RFC, unless the revision is large enough to warrant its own document.
