# Feature Requests / RFCs

Design proposals for upcoming TinyBooth Sound Studio features. Each RFC follows a consistent shape: header table, Executive summary, Problem, Proposal, Implementation notes, Risks, Open questions, Success criteria. Quote the **Session serial** when asking for revisions to keep continuity with the underlying research.

| ID | Title | Status | Landed in | Notes |
|---|---|---|---|---|
| **TBSS-FR-0001** | [Suno cleanup mode](TBSS-FR-0001-suno-cleanup.md) | ✅ Implemented (DSP) | v0.1.6 | Parametric EQ + de-esser added to FilterChain(Stereo); Suno-Clean preset shipped. Mix-tab path delivered by FR-0002. |
| **TBSS-FR-0002** | [Multitrack remastering](TBSS-FR-0002-multitrack-remastering.md) | ✅ Implemented | v0.2.0 | Mix tab + cpal output player + per-track corrections + A/B bypass + correction-aware export. |
| **TBSS-FR-0003** | [Import normalization](TBSS-FR-0003-import-normalization.md) | 📝 Proposed | — | LUFS-balance imported stems on ingest; non-destructive (gain only). |
| **TBSS-FR-0004** | [Console mixer + volume automation](TBSS-FR-0004-console-mixer-automation.md) | 📝 Proposed | — | Hardware-style fader strips on the Mix tab + timestamped fader-movement automation per studio-console convention. |

## Convention

- File name: `TBSS-FR-NNNN-kebab-case-title.md`. Numbers monotonic, never reused.
- Status values: `📝 Proposed`, `🔧 In progress`, `✅ Implemented`, `⛔ Withdrawn`.
- Once an RFC is implemented, the doc stays in this folder as the historical record. The header table's "Status" line is updated; the body is left as-was so the design rationale at decision time is preserved.
- A revision (e.g. FR-0001's §7 was rewritten when Suno's server-side stems made local separation obsolete) is annotated inline rather than splitting into a new RFC, unless the revision is large enough to warrant its own document.
