# Codebase audit — 2026-04-27 (v0.3.5)

Clinical pass over `src/` — bugs, smells, performance opportunities, missing tests, refactor candidates. Findings are categorised and severity-ranked. Most are not urgent; this is the kind of doc that gets revisited at version-bump rest stops.

For the live architecture, see [`docs/architecture.md`](../architecture.md). For Rust patterns broadly, see [`docs/rust-survival-guide.md`](../rust-survival-guide.md).

## Methodology

Read every `src/*.rs` and `src/ui/*.rs` file end-to-end. Spot-checked dependency interactions. No `cargo clippy --release -- -D warnings` run yet — see §10 for what that would catch automatically.

Each finding is tagged:

- **Bug** — actually wrong; can mis-behave under conditions a user might hit.
- **Smell** — design choice that compounds maintenance cost; not wrong today.
- **Perf** — runs correctly, runs slower than it could.
- **Test** — missing test coverage for non-trivial logic.
- **Doc** — internal documentation gap.

And severity:

- **Critical** — fix before next release.
- **Major** — fix soon (before v0.4).
- **Minor** — worth a tidy when touching the file.
- **Nit** — preference / consistency.

---

## 1. Top-line findings

| # | Finding | Tag | Severity |
|---:|---|---|---|
| 1 | Zero automated tests anywhere in the repo. | Test | **Major** |
| 2 | `git_update.rs::render` calls `std::process::exit(0)` on download success. | Smell | **Major** |
| 3 | Profile editor duplicated across `ui/admin.rs` and `ui/correction.rs`. | Smell | **Major** |
| 4 | `export.rs::mixdown` uses an unintuitive gain-undo trick (`db_to_lin(g) / db_to_lin(static)`). | Smell | Minor |
| 5 | `cargo clippy` not in CI. | Smell | Minor |
| 6 | Default scratch project path `%APPDATA%\…\sessions\session-<ts>\` accumulates indefinitely. | Smell | Minor |
| 7 | `git_update.rs` panics quietly on rapid clicks (extra background threads spawned per click). | Smell | Nit |
| 8 | `Profile` cloned freely (~10× per Suno-Clean Enable-all on a 9-stem project). | Perf | Nit |
| 9 | `audio.rs::start_recording` has triple `match sample_format` boilerplate. | Smell | Nit |
| 10 | `cpal` and `hound` errors mapped to `String` in `git_update.rs` instead of `anyhow::Error`. | Smell | Nit |

The blocking ones are #1 (no tests) and #2 (hard exit on update path). Everything else is taste.

---

## 2. File-by-file

### 2.1 `src/main.rs` (58 LOC)

Entrypoint. No issues. The viewport-icon `load_icon()` returns `IconData::default()` on decode failure — that's the right choice (graceful) and is documented.

### 2.2 `src/app.rs` (727 LOC) — central state

#### Smells

- **The struct has 25+ fields**. State that logically clusters (recording, playback, modal flags, update plumbing) is splayed flat. A future refactor could group:
  ```rust
  pub struct TinyBoothApp {
      pub config: Config,
      pub project: ProjectState,        // project + dirty + recent
      pub recording: RecordingState,    // devices + selected + session + viz
      pub playback: PlaybackState,      // player + recorder + correction-edit
      pub modals: ModalState,           // all the Option<...>'s for floating windows
      pub updater: UpdateState,         // self-update plumbing
      pub view: ViewState,              // tab, status, manual
  }
  ```
  Not urgent — the flat layout works — but the next time this file grows past 1,000 LOC it'll be the right move. **[Smell, Minor]**

- **Method ordering is chronological, not logical.** Methods are appended as features land. After v0.3.4 the file reads as a stratigraphy of release notes. Reorder by feature cluster the next time you touch it. **[Smell, Nit]**

#### Other observations

- `apply_import_outcome` is a good consolidation point — both folder and zip imports go through it. Keep this pattern when adding more import sources.
- `enable_all_corrections` cascade logic (project default → Suno-Clean → first profile) is right; tested by hand.
- Borrow-checker pattern (`click_*` locals fired after closures) is consistent throughout. Good.

### 2.3 `src/audio.rs` (388 LOC) — recording

#### Smells

- **Triple `match sample_format` boilerplate** at lines 251-258 and again in the mono branch at 261-268. Six near-identical lines per branch. A small generic helper would collapse them:
  ```rust
  fn dispatch_sample_format<F, R>(fmt: SampleFormat, f: F) -> Result<R>
  where F: FnOnce(SampleFormat) -> Result<R>
  ```
  Minor improvement; the current form is at least readable. **[Smell, Nit]**

- **The cpal `err_fn` just `eprintln!`s** the error. A real user running from a double-clicked icon never sees it. Plumb to the UI thread via a channel so audio errors appear in the status bar. **[Smell, Minor]**

#### Tests missing

- `SourceMode::Stereo` rejection when device has 1 channel — easy unit test against a mocked device. **[Test, Minor]**

### 2.4 `src/automation.rs` (139 LOC) — splines

Solid. No issues.

The pad-endpoints trick in `SplineSampler::build` is correct and worth a comment that's already there.

### 2.5 `src/config.rs` (86 LOC)

#### Smells

- **`Config::save` swallows every error silently**. If `dirs::config_dir()` returns None or write fails (full disk, permission), the user sees no indication their settings didn't persist. Minimum: log to stderr. Better: surface to status bar via a `Result`. **[Smell, Minor]**

- **No file locking around config write.** If TinyBooth is launched twice and both write `config.json`, last write wins. Race window is tiny but a `.tmp` + rename pattern is the standard fix. **[Smell, Nit]**

### 2.6 `src/dsp.rs` (639 LOC) — Profile + filter chains

#### Smells

- **`builtin_profiles()` constructs five hand-written profiles plus calls a closure factory `rec(...)`** for the recording-tone four. That closure factory takes 14 positional parameters, which is slightly wild. A builder pattern (`Profile::recording_tone().name(…).hpf(60.0).…build()`) would read better and be easier to extend. Not pressing — five profiles isn't going to grow. **[Smell, Nit]**

- **`FilterChain` and `FilterChainStereo` duplicate ~60% of their bodies** (gate envelope follower, compressor envelope follower). The mono and stereo versions could share via a trait `EnvelopeFollower` that the gate and compressor parameterise on. Save code at the cost of slightly more reading. Not worth it for the size we have. **[Smell, Nit]**

#### Tests missing

- `is_newer` semver comparator (in `git_update.rs` actually, but property-style): `parse(latest) > parse(current)`. Property test on random version strings would catch off-by-one decade. **[Test, Minor]**

- Filter coefficients for known inputs (HPF at 100 Hz on a 48 kHz signal): sanity-check the magnitude response. Snapshot test. **[Test, Minor]**

### 2.7 `src/export.rs` (322 LOC) — mixdown

#### Smell — the gain-undo trick

Lines 161-164:
```rust
let gain_db = auto_sampler.as_ref()
    .and_then(|s| s.sample(f as f32 / sr_f))
    .unwrap_or(static_gain_db);
let g = db_to_lin(gain_db) / db_to_lin(static_gain_db); // raw was scaled by static gain
let l_g = l * g;
```

The raw samples were pre-scaled by `db_to_lin(t.gain_db)` during the read step (line 110-117 of the existing implementation). To switch to the automation-driven gain we *divide out* the static and *multiply in* the automation. Mathematically correct, but the comment is ten lines explaining why a division shows up. **Refactor**: don't pre-multiply during read. Read raw, apply gain in the per-frame loop. The export gets simpler:

```rust
let raw_unscaled = ...; // no gain in here
for f in 0..frame_count {
    let (l, r) = ...;
    let (l, r) = chain.process(l, r);
    let gain_db = auto_sampler.sample(t).unwrap_or(static_gain_db);
    let g = db_to_lin(gain_db);
    buf.push(l * g); buf.push(r * g);
}
```

Three lines deleted, one ten-line comment removed, no behaviour change. **[Smell, Minor]**

#### Tests missing

- Round-trip: build a known project (one mono track, one stereo track, fixed gain, no correction), export, read back, assert sample equality. **[Test, Major]**

- ffmpeg path discovery (`find_ffmpeg`): mock filesystem, exercise the three search paths. **[Test, Minor]**

### 2.8 `src/git_update.rs` (156 LOC) — self-update

#### Major: hard exit on success

Line 111: `Ok(_) => std::process::exit(0)`. This kills the process the moment `download_and_install` returns Ok. Problems:

1. No `Drop` impls run. `WavWriter` flushes its header on drop; if the user is recording when the update completes (unusual but possible — they could have started a take while the download was in flight), the take is corrupted.
2. egui's `eframe::run_native` doesn't get its shutdown sequence; the GLOW context is leaked.
3. Config doesn't get its final save.

**Fix**: use `frame.close()` on the eframe Frame to request a clean shutdown, then return. Plumbing-wise that means `git_update::render` needs a `&mut eframe::Frame` parameter or signals back to `app.rs` which owns the frame. **[Smell, Major]**

#### Other

- `download_and_install` returns `Result<PathBuf, String>` instead of `anyhow::Result<PathBuf>`. The String alternative loses error context (no chain). Convert. **[Smell, Nit]**

- Rapid-clicking the version label spawns a fresh thread per click before the first one has settled. The `matches!(state, UpdateState::Idle)` check guards the *click* but not the *thread spawn race* — between click and `*state = Checking` being set, another click could fire. Tiny window; not worth fixing. **[Smell, Nit]**

#### Tests missing

- `is_newer` comparator: trivial property test. **[Test, Minor]**

### 2.9 `src/manual.rs` (125 LOC)

Solid. Page list is plain data; `find()` is one-line linear search (12 entries, fine).

### 2.10 `src/player.rs` (598 LOC) — playback

This was just refactored in v0.3.4 Phase B. Per-buffer cache, scratch buffers pre-allocated, atomic loads minimised. No outstanding findings on the audio thread.

#### Smells

- **The `SAFETY: has_chain == true ⇒ chains[i] is Some` invariant** is enforced by hand. The cache could carry the chain pointer directly to eliminate the `unwrap`:
  ```rust
  struct TrackBufCache<'a> {
      chain: Option<&'a mut FilterChainStereo>,
      ...
  }
  ```
  But borrowing across the cache would require restructuring the `chains` storage. Not pressing. **[Smell, Nit]**

#### Tests missing

- `compute_peaks` against a known waveform (sine, silence, full-scale). **[Test, Minor]**

- `read_frame` boundary cases: pos at frame_count, pos > frame_count, mono vs stereo. **[Test, Minor]**

### 2.11 `src/project.rs` (245 LOC) — schema

#### Smells

- **`Track::new(...)` doesn't exist.** Every Track is constructed by literal field-by-field expression at five different sites (`app.rs::start_new_take`, `suno_import.rs::build_project`, the Skeleton bootstrap, etc.). Adding a field means touching all sites — and forgetting any one is a silent breakage caught only at compile time (good) or at runtime (the field stays default). A constructor helper would centralise this:
  ```rust
  impl Track {
      pub fn recorded(id: &str, name: &str, file: &str, sample_rate: u32, ...) -> Self
      pub fn from_suno_stem(...) -> Self
  }
  ```
  **[Smell, Major]** — every schema addition forgets to land in one of the sites. We've caught all of them with `cargo check` so far, but it's a pattern that gets worse as fields grow.

#### Doc gaps

- The relationship between `Track.profile` (recording-tone snapshot) and `Track.correction` (post-processing chain) isn't called out at the type level. Both are `Option<Profile>`. Same shape, different semantics. A doc comment on each pointing to the other. **[Doc, Minor]**

### 2.12 `src/suno_import.rs` (754 LOC) — Suno ingestion

The biggest file. Multiple distinct responsibilities mixed:

1. Folder ingestion path
2. Zip ingestion path
3. Filename → `StemRole` matcher (`match_role`)
4. Tempo-Locked filter (`is_tempo_locked`)
5. WAV header read
6. Per-import log file management (`ImportLog`)
7. Pre-import probe (`probe_folder` / `probe_zip`)
8. Project wipe (`wipe_project_root`)

#### Smells

- **Should be split into 2-3 files**. `suno_import/` with `mod.rs`, `log.rs`, `probe.rs`, `roles.rs` would be more navigable. The 754-line single file requires you to scroll past three other concerns to find the matcher. **[Smell, Major]**

- **`import_folder` and `import_zip` duplicate ~80% of their bodies** (the per-entry decision tree). The differences are how to enumerate entries and how to extract a single one. A trait `ImportSource { fn entries(...); fn extract(...); }` would let one function handle both. Risk: more abstraction. Win: one decision tree to maintain. Worth it for a file this size. **[Smell, Major]**

#### Bugs

- **`first_session_in_zip` extracts every WAV until it finds one with metadata**. If the first WAV doesn't have ICMT, we extract the next, etc. We extract to a temp file, read, delete. For a 12-stem 30 MB-each bundle that's potentially 360 MB of disk churn just to probe. Optimise by reading only the header chunks (RIFF + LIST), not the full file. The full extraction was needed before our RIFF walker existed. **[Perf, Minor]** — plus a slight correctness concern: if the temp file isn't deleted on a panic between extract and `read_wav_session`, we leak files in temp.

#### Tests missing

- `match_role` is pure-function string matching with 16 rules. Worth a table-driven test:
  ```rust
  #[test]
  fn match_role_table() {
      assert_eq!(match_role("backing_vocals.wav"), StemRole::BackingVocals);
      assert_eq!(match_role("Drums.wav"), StemRole::Drums);
      assert_eq!(match_role("electric_guitar_2.wav"), StemRole::ElectricGuitar);
      // ... ~30 cases
  }
  ```
  **[Test, Major]**

- `is_tempo_locked` is one-liner; trivial test. **[Test, Minor]**

### 2.13 `src/suno_meta.rs` (128 LOC) — RIFF reader

Tight, focused. Walks RIFF chunks, finds LIST/INFO/ICMT, parses `created=<ISO>`.

#### Tests missing

- This is the most testable file in the repo. Generate fixture WAVs with known ICMT contents, parse, assert. **[Test, Major]**

### 2.14 `src/ui/admin.rs` (227 LOC) and `src/ui/correction.rs` (194 LOC) — Profile editors

#### Major: duplication

These two files render the same `Profile` (Input gain → HPF → 4-band EQ → de-esser → Gate → Compressor → Makeup) with slightly different framing. Compare:

`admin.rs` lines 130-200 and `correction.rs` lines 80-170. ~70% byte-overlap.

#### Suggested refactor

Extract `pub fn render_profile_editor(p: &mut Profile, ui: &mut egui::Ui) -> bool` (returns `changed`) into either `dsp.rs` (with a `pub mod ui;` submodule) or a new `ui/profile_editor.rs`. Both Admin and Correction use it.

The framing differences are Admin's "+ New / Save all / Reset" vs Correction's "Disable / Enable with Suno-Clean" — those stay in the respective modules. **[Smell, Major]**

### 2.15 `src/ui/mix.rs` (610 LOC) — Mix tab

#### Smells

- **`transport_bar`, `lanes_view`, `console_deck`, `strip`, `master_strip` all live in one file**. Split into `ui/mix/` with each as its own file. **[Smell, Minor]**

- **STRIP_W, FADER_H, METER_W, STRIP_GAP** are file-level constants at the top. Some are only used by `strip`, others only by master. Co-locate them with their consumers when splitting. **[Smell, Nit]**

### 2.16 `src/ui/import_dialog.rs` (79 LOC) and `src/ui/import_conflict.rs` (98 LOC)

Both are modal-with-buttons-and-text. Mostly distinct content, so duplication isn't wasteful. Could share a `centered_modal(ctx, title, body, buttons)` helper if we add a third modal. **[Smell, Nit]**

### 2.17 Other UI files

- `ui/manual.rs`, `ui/record.rs`, `ui/project.rs`, `ui/export.rs`, `ui/viz.rs` — all reasonably sized, no major issues.
- `ui/mod.rs` — minimal `pub mod` declarations. Good.

---

## 3. Cross-cutting concerns

### 3.1 Tests (Major)

**There are no tests.** Zero `#[test]` functions. Zero `tests/` directory. `cargo test` runs and passes — because there's nothing to fail.

This is the single highest-leverage finding. The functions most worth covering, in priority order:

1. `match_role` — 16 substring rules, 100% pure, easy table-test.
2. `suno_meta::parse_icmt` and `read_wav_session` — fixture-driven.
3. `automation::SplineSampler::sample` — boundaries, empty lane, single point, multi-point.
4. `project::{save, load}` round-trip on a fixture manifest.
5. `git_update::is_newer` semver comparator.
6. `analysis::peak_bins` and `analysis::spectrum` boundary cases.
7. `export::mixdown` against a fixture project — most valuable, hardest to write.
8. `dsp::FilterChain::process` against known input/expected output (sine through HPF, etc.).

A full afternoon would land 1-6. Item 7 is a half-day. Item 8 is a day. If you do nothing else from this audit, do tests.

### 3.2 No clippy in CI

The release workflow doesn't run `cargo clippy`. Adding it as a pre-build step is one line:

```yaml
- run: cargo clippy --release -- -D warnings
```

It would catch:

- The `unused import` and `dead code` warnings we suppress with `#[allow]`.
- Any future regressions.
- A handful of clippy-specific idioms we're not using consistently (`.iter().for_each` vs `for _ in`, etc.).

Run `cargo clippy --release -- -D warnings` locally first to confirm a clean baseline before adding to CI. **[Smell, Minor]**

### 3.3 No CHANGELOG

Releases get auto-generated notes from commit messages, which works but reads as a wall of `feat:`/`fix:` if anyone scrolls. A handwritten `CHANGELOG.md` keyed by version is the canonical answer. Keep-a-Changelog format is fine. **[Doc, Minor]**

### 3.4 Dependency review

```
eframe         0.28   ← stable, mature, ~2 years
egui           0.28   ← (transitively pinned to eframe)
egui_commonmark 0.17  ← Markdown rendering for the in-app manual
serde          1.x    ← never moves
serde_json     1.x    ← never moves
dirs           5      ← stable
rfd            0.14   ← native dialogs, stable
anyhow         1      ← never moves
parking_lot    0.12   ← stable, faster than std::sync
chrono         0.4    ← stable, large
reqwest        0.12   ← stable; default-features = false ✓
cpal           0.15   ← stable
hound          3.5    ← WAV reader; hasn't moved in years
rustfft        6.x    ← stable
biquad         0.5    ← tiny, stable
image          0.25   ← png-only feature ✓
zip            2      ← deflate-only ✓
open           5      ← OS file-opener
splines        5      ← Catmull-Rom replay
winres         0.1    ← Windows resource embed
```

No red flags. All MIT/Apache-2.0 dual or compatible. No unmaintained crates. `cargo audit` would confirm no known advisories — worth running monthly.

`reqwest`, `image`, and `zip` all have `default-features = false`, the right call. `chrono` doesn't — could trim with `default-features = false, features = ["clock", "serde", "std"]` to save ~20 KB and a few transitive deps. Not urgent.

### 3.5 `unsafe` audit

Zero `unsafe` blocks anywhere in `src/`. Confirmed via `grep "unsafe" src/`. Good.

### 3.6 Documentation

- **`docs/manual/`** — twelve user-facing chapters. Comprehensive. Includes the in-app version (via `include_str!`).
- **`docs/feature-requests/`** — formal RFCs for completed and proposed features.
- **`docs/architecture.md`** — landing today.
- **`docs/rust-survival-guide.md`** — landing today.
- **`docs/audit/`** — this doc, plus future ones.
- **No rustdoc deployment**. `cargo doc --no-deps` would generate API docs. Worth running locally; not worth hosting on GH Pages until the codebase stabilises.

---

## 4. Suggested patches (ordered by ROI)

### 4.1 Land these first (high value, low cost)

1. **Add `cargo clippy --release -- -D warnings` to CI**. ~20 min including a local clean-up pass.
2. **Write `match_role` table tests**. ~30 min. Catches future regression on filename-substring rules.
3. **Add `is_newer` and `parse_icmt` tests**. ~30 min total. Both are pure-function.
4. **Refactor `git_update::render` to use `frame.close()` instead of `process::exit`**. ~1 hour including plumbing the `Frame` reference. Removes the fast-update-during-record corruption risk.
5. **Extract `render_profile_editor` shared helper from `ui/admin.rs` and `ui/correction.rs`**. ~2 hours. ~150 LOC consolidation.

### 4.2 Land these soon (medium value, medium cost)

6. **`Track::recorded(...)` and `Track::from_suno_stem(...)` constructors**. ~1 hour. Removes the field-list-fanout problem for future schema additions.
7. **Refactor `export.rs::mixdown` to apply gain in the per-frame loop, not pre-multiplied during read**. ~1 hour. Removes the gain-undo trick + ten-line apologetic comment.
8. **Round-trip serialisation tests** for `Project` and `Profile` against fixture manifests. ~3 hours including fixture authoring. Catches schema breakage on every PR.
9. **`suno_import.rs` split into `suno_import/` submodule** with `log.rs`, `probe.rs`, `roles.rs`. ~3 hours. Improves navigability of the largest file.

### 4.3 Land these eventually (lower priority)

10. **Group `TinyBoothApp` fields into nested structs** (RecordingState, PlaybackState, ModalState, …). ~half day. Improves readability when the file passes 1000 LOC.
11. **Trait-based `ImportSource` collapsing folder + zip duplication**. ~half day. Cleaner architecture; risk of over-abstraction.
12. **`FilterChain` / `FilterChainStereo` shared envelope follower**. ~half day. Trade-off — saves code at cost of slightly indirect reading.

---

## 5. Suggested missing features (out of scope for refactor, worth filing)

These came up during the audit as natural extensions worth their own RFCs eventually:

- **`tools/compare.py` integration into the app.** A "Compare to baseline" button on the Export tab that runs the same metrics in-process and shows the report in a modal. RFC-worthy.
- **Per-stem correction-preset library**. Save / load named correction chains by name (Vocals-Clean, Drums-Clean, etc.) — already flagged as Phase 3 of TBSS-FR-0002.
- **Click-to-seek on Mix-tab waveform lanes** — already flagged as Phase 3.
- **Loop region playback** — record a section, loop it while you ride a fader.
- **Solo button on master strip currently no-ops** — fine, but a small visual indicator would help.

---

## 6. Closing read

Codebase is in good shape for its age and feature set. The big-ticket items (audio-thread allocation hygiene, schema migration discipline, modal isolation pattern, generation-counter rebuild) are all sound. The risks are concentrated in two areas:

1. **No tests.** A regression in any pure-function — `match_role`, `is_newer`, `parse_icmt`, `compute_peaks` — would ship silently. Item #1 in priority.
2. **`process::exit` on update.** A user updating mid-record loses the take. Item #4 in priority.

Everything else compounds slowly. Address these two and pick from §4.1 / §4.2 at your release-rest cadence.

---

## Revision history

| Date | Audit | Tag at audit time |
|---|---|---|
| 2026-04-27 | this document | v0.3.5 |
