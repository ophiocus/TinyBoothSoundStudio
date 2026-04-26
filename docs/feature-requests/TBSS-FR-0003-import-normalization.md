# TBSS-FR-0003 — Import normalization

| Field | Value |
|---|---|
| **Request ID** | TBSS-FR-0003 |
| **Title** | Loudness-balance imported stems on ingest (LUFS-based, non-destructive) |
| **Status** | Proposed |
| **Date filed** | 2026-04-26 |
| **Author** | Claude (authoring assistant) on behalf of project owner |
| **Session serial** | `CLAUDE-2026-04-26-NORMALIZE-RFC-0003` |
| **Depends on** | TBSS-FR-0001 (Suno cleanup substrate, ✅ landed v0.1.6); TBSS-FR-0002 (Mix tab, ✅ landed v0.2.0) |
| **Breaking changes** | None to users; `Track` schema unchanged. |

---

## 1. Executive summary

When the Suno-bundle ingester drops a 12-stem bundle into a TinyBooth project today, every `Track.gain_db` is set to `0.0`. The relative levels are whatever Suno baked into the source files — which can be wildly inconsistent: vocals at ~−6 dB peak alongside backing pads at ~−18 dB, drums hot enough to clip the bus the moment all stems play together.

Add a **non-destructive loudness balance pass at import time** that measures each stem's integrated LUFS and writes a per-track gain offset into the manifest. The user lands in the Project tab with stems already at a comfortable starting balance — Suno's relative mix is preserved (so the bass still sits where Suno wanted it relative to the kick), but the absolute level is sane.

**Crucially: the WAV files are never modified.** The "intermediate WAV private to the project" question raised in the spec is answered "no" — gain is metadata, lives in `.tinybooth`, and the source copy under `tracks/` stays bit-exact to whatever Suno delivered.

## 2. Problem

Today, after `File → Import Suno stems → from zip`:

- Every Track has `gain_db = 0.0`.
- Mix tab playback sums all stems at unity → bus pegs at +6/+9 dB peaks → soft-limit kicks in immediately → a flat-but-distorted starting mix.
- User has to drag each track's gain slider down before they can hear anything resembling the Suno render.
- Suno's intra-mix relative balance (kick vs. snare vs. bass etc.) is fine; it's the absolute starting bus level that's wrong.

The user-flagged remediation: have the importer pre-set `track.gain_db` so the bus is in the right ballpark on first play.

## 3. Proposal

### 3.1 Scope

**Loudness normalization at import**, applied to each kept WAV after the WAV header read but before the project manifest is finalised. Uses integrated LUFS (BS.1770) as the reference measure; writes a `gain_db` value to each Track. Source WAVs are not transcoded, resampled, or otherwise altered.

A **Settings toggle** (`normalize_on_import: bool`, default `true`) governs whether the pass runs. Default-on because the better-balanced starting state is what most users want; opt-out for users who want to preserve raw imported levels.

### 3.2 Algorithm

For each kept stem:

1. Open the freshly-extracted WAV with `hound`.
2. Stream samples into the `ebur128` analyser (BS.1770 R128 integrated loudness). Whole-file pass, single channel sum (or stereo if `track.stereo`).
3. Record the integrated LUFS.

After all stems are measured:

4. Compute the **reference LUFS** = `max(integrated_lufs)` across stems (i.e. the loudest stem in the bundle — usually a Master or Vocals stem).
5. Compute **target LUFS** = a project-wide constant. Default: `−16 LUFS` (a reasonable mix-bus target that leaves headroom for further processing). Configurable in Settings.
6. For each stem:
    `track.gain_db = target_lufs − stem_lufs + (reference_lufs − target_lufs)`
   which simplifies to `target_lufs + reference_lufs − 2·stem_lufs` — but the more readable form: bring the loudest stem to target, drop the others by their distance from the reference. Suno's relative mix is preserved.

   Edge case: if `stem_lufs` is `-inf` (silence), set `gain_db = 0.0` and continue.

### 3.3 Non-destructive guarantee

- WAV files under `<project>/tracks/` are written by `import_zip` / `import_folder` exactly once (the extraction copy) and never re-opened for write.
- `track.gain_db` is the single source of truth for level. Resetting (gain = 0 in Project tab or the new Console mixer from FR-0004) returns the track to its raw imported level.
- The original Suno zip / folder is never touched by anything in the import path.

### 3.4 No "intermediate WAV"

The spec asked: *"should import result in an intermediate WAV private to the project?"* Two interpretations:

| Interpretation | Decision | Rationale |
|---|---|---|
| **Loudness-flattened intermediate** (rewrite each WAV with the gain baked in) | **No** | Loses the "reset to raw" capability. Doubles disk usage if we keep both. Gain in metadata is reversible and cheap. |
| **Format-normalised intermediate** (transcode all stems to a single canonical sample rate / bit depth) | **No, for now** | All Suno stems already share a sample rate — the player and exporter already require this. If a future bundle mixes rates, the player will error out clearly; we'll add an opt-in resampling pass then. Tracked separately as a Phase-3 polish item. |

If a user wants to make the gain decisions permanent, the existing **Export** tab already produces a rendered WAV with all corrections and gains baked in — that's the canonical "flatten" path.

## 4. Implementation

### 4.1 New dep

```toml
ebur128 = "0.1"   # MIT — pure Rust port of libebur128
```

`ebur128` is mature, single-purpose, no system deps. The same crate is referenced as Phase A in TBSS-FR-0001 §7 (LUFS metering) so adding it now also unlocks that future feature for free.

### 4.2 Changes

- `src/suno_import.rs` — after `read_wav_meta` succeeds, run a second pass that streams the file's PCM into an `ebur128::EbuR128` analyser and reads `loudness_global()`. Stash the result on the `Detected` struct alongside `sample_rate`/`channels`.
- New helper `compute_balance_gains(detected: &mut [Detected], target_lufs: f32)` that fills in each detected entry's pre-computed gain, run inside `finalize` once we know the per-stem LUFS values.
- `build_project` writes `gain_db` from the precomputed value instead of the current `0.0`.
- `Config` gains two new `#[serde(default)]` fields:
  - `normalize_on_import: bool` (default `true`)
  - `import_loudness_target_lufs: f32` (default `-16.0`)
- The import log file gets two extra columns per kept entry: `lufs=-12.3` and `gain=-3.7dB`.
- The import-result modal's summary text gains a one-line "Balanced to −16 LUFS reference" notice.

### 4.3 UI surface

A new section in the existing **Admin** menu (or a new `Settings…` window — TBD; lightweight enough to fit in Admin):

- "Normalize stems on import" — checkbox.
- "Loudness reference (LUFS)" — number input, range `-30.0..=0.0`.

The Project tab's per-track gain slider is unchanged — it now starts populated rather than at 0, but is still freely editable.

### 4.4 Lift

| Item | LOC | Time |
|---|---|---|
| ebur128 integration in import path | ~80 | ½ day |
| Config fields + Admin/Settings UI | ~60 | ~2h |
| Log + modal annotation | ~20 | ~30min |
| Manual chapter update | ~30 | ~30min |
| **Total** | **~190** | **~1 day** |

## 5. Risks & mitigations

| Risk | Mitigation |
|---|---|
| LUFS measurement is slow on long tracks (a 5-minute stereo file processed sample-by-sample takes ~0.5–1 s) | Run the measurement pass off the UI thread (already the case — import runs in the menu callback). The added latency on a 12-stem bundle is ~5–10 s; surface progress in the existing log + modal. Optionally parallelise across stems with `rayon` if profiling shows it matters. |
| Silent stems return −inf LUFS, wrecking the reference computation | Special-case `f64::is_finite` on the measurement; treat silent stems as `gain_db = 0.0` and exclude them from the reference. |
| User wants to suppress the balance for one specific bundle | Settings toggle is global, but the per-track gain slider always overrides — drag any track back to 0 dB and the raw level is restored. |
| Default target of −16 LUFS surprises users who expect louder/streaming-style output | Document the default, expose it as a setting, and explain the streaming-style alternative (−14 LUFS) and broadcast (−23 LUFS). The Export tab will get its own LUFS target later (FR-0001 §7 Phase A) — that's the right place to hit a release-spec loudness, not import time. |

## 6. Open questions

1. Should we also surface the measured per-stem LUFS in the Project tab (read-only column) so users see what the balance was based on? Tiny addition; useful for "why is this stem so loud?" questions.
2. Stereo stems: measure as integrated stereo (BS.1770 standard), or as `(L_lufs + R_lufs) / 2`? BS.1770 is the right answer; ebur128 handles it natively.
3. Does the reference want to be the loudest stem or a specific role (e.g. always Master if present)? "Loudest" is more robust to bundles that don't include a Master stem; "Master if present, else loudest" is an easy refinement.

## 7. Success criteria

- After importing a 12-stem Suno bundle, hitting Play in the Mix tab produces a mix whose bus peak sits within −9 to −3 dB on typical material — no soft-limit engagement, no manual gain riding required.
- Resetting any track's gain to 0.0 returns its level to the raw Suno output (verified by null-test against the unprocessed WAV).
- Projects whose manifests pre-date this feature load unchanged — `gain_db` defaults preserved, no normalization applied retroactively.
- Disabling the Settings toggle and re-importing the same bundle yields the v0.2.2 behaviour (all gains 0).

## 8. Out of scope

- Per-track LUFS metering during playback (separate FR — sits naturally inside the Console mixer in FR-0004's master strip).
- Per-export loudness target (covered by FR-0001 §7 Phase A — LUFS metering + target normalize on export).
- Format normalization (resample / re-bit-depth) on import — deferred to Phase-3 polish; gated on a real complaint about mismatched-rate bundles.

---

*Session serial `CLAUDE-2026-04-26-NORMALIZE-RFC-0003`. Quote when requesting revisions that should build on this design.*
