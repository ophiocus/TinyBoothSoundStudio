# Importing Suno stems

Suno's Pro and Premier tiers can extract per-track stems from any song you generated. TinyBooth ingests those stems directly — drop a folder or zip in, get a TinyBooth project where each stem is its own track.

This replaces what would have been a local stem-separation feature. Suno already separates server-side; reseparating locally is wasted work.

## Two entry points

- **File → Import Suno stems → …from folder** — pick a directory containing the per-stem WAV files Suno gave you.
- **File → Import Suno stems → …from zip** — pick the "Download All" zip Suno produces, no need to unzip first.

A new TinyBooth project is created as a sibling of the source (same parent folder), named `<source> (TinyBooth)`. The app switches to the Project tab with everything ready.

## What gets imported

- **`.wav` files only.** MP3 ingestion is not supported in this version — re-download as WAV from Suno if you have an MP3 bundle.
- **Tempo-Locked WAVs are skipped.** Suno offers these as time-stretched-to-average-BPM variants; they will not sum back to the original master, so importing them would be misleading. The ingester filters by filename hint (`tempo*lock`).
- **WAV headers are read for ground truth.** Sample rate, bit depth, channel count come from the file header — never from the filename — because Suno's filename schema is not officially documented.

## How stems are tagged

Each imported track gets a `StemRole` derived from a case-insensitive substring match against its filename:

| Filename hint | Role |
|---|---|
| `vocal` + `back` | Backing Vocals |
| `vocal` (else) | Vocals |
| `drum` | Drums |
| `bass` | Bass |
| `electric` + `guitar` | Electric Guitar |
| `acoustic` + `guitar` | Acoustic Guitar |
| `guitar` (else) | Electric Guitar (generic fallback) |
| `piano` or `key` | Keys |
| `synth` or `lead` | Synth / Lead |
| `pad` or `chord` | Pads / Chords |
| `string` | Strings |
| `brass` or `wood` | Brass / Wind |
| `perc` | Percussion |
| `fx` or `other` | FX / Other |
| `instrumental` | Instrumental (legacy 2-stem) |
| `master`, `mix`, `final` | Master |
| (anything else) | Unknown |

The matcher is intentionally permissive. Suno doesn't publish a schema, so contractual matching would break the moment they renamed a stem. Numeric suffixes like `drums_1.wav` and `drums_2.wav` are handled implicitly — each becomes its own track with a unique filename inside `tracks/`.

## After import

You're now in a normal TinyBooth project. The Project tab source column shows each row as `Suno · Vocals`, `Suno · Drums`, etc. You can mute / rename / set gain / delete just like a recorded take. The original Suno filename is preserved in the manifest for reference but not displayed.

Future work — the planned **Clean** tab — will dispatch role-aware processing on these stems (e.g. de-esser only on vocals, drum bus glue only on percussion). For now, the import is a structural unlock; the per-role processing is documented under TBSS-FR-0001 in the source repo.

## Session metadata (epoch + ordinal)

Suno stamps every stem WAV with a `LIST/INFO/ICMT` RIFF comment that reads like `made with suno studio; created=2026-04-25T05:31:37Z`. The ingester reads this on every kept stem and stores:

- **`session_epoch`** (Unix integer seconds) — identical across all stems of one Suno render, distinct between re-renders. JSON-clean, sortable directly.
- **`session_ordinal`** — a project-relative import counter (1, 2, 3, …). Bumped on every successful import; all tracks from one import event share the ordinal.
- **`provenance`** — the free-form prefix from the same ICMT chunk.

These appear in the **Project tab** as `Suno · Vocals (#1)`, `Suno · Drums (#1)` etc. — hover the source column for the full epoch / ISO / provenance triple.

## Duplicate-import detection

Re-importing a bundle whose `session_epoch` already exists in the target project's manifest triggers a confirmation modal: **Replace existing project** or **Cancel**. Replace wipes the old `tracks/` and manifest, re-imports fresh, and assigns a new `session_ordinal`. Cancel leaves everything as it was. (Pre-v0.3.1 the re-import quietly accumulated `-002` / `-003` collision-renamed copies — this is the explicit fix.)

If you want to keep the old project alongside the new import, **rename the existing project folder before re-importing**. The detector looks at the proposed target root only; a different folder name produces no conflict.

## Limitations

- No online stem fetch. Suno has no official API; community wrappers break when Suno rotates auth, so TinyBooth deliberately doesn't ship one. Manual zip-drop is the supported path.
- Stems are **not** guaranteed to sum bit-exactly to the rendered Suno master. Mastering is applied to the master only. Expect small residuals.
- Few-sample timing offsets between stems have been reported by the Suno community on rare tracks. TinyBooth doesn't auto-correct; nudge gain/delay manually in the Project tab if needed.
