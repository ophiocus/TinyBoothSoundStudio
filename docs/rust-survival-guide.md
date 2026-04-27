# Rust survival guide for industrial-grade applications

A pragmatic field manual for building shipping-quality Rust desktop apps — written from the experience of building TinyBooth Sound Studio but applicable broadly. Not a Rust-language tutorial. Assumes you know `&`, `Box`, `Result`, `match`. Aims at the tier of decisions you make AFTER you've stopped fighting the borrow checker every five minutes.

Intended audience: someone shipping a Rust binary to real users who will file bugs, demand updates, run it on hardware you've never seen, and not give a damn how clever your trait bounds are.

---

## 1. Project structure

### 1.1 Single binary or workspace?

**Default: single `[bin]` crate.** Reach for a Cargo workspace only when:

- You actually have a reusable library to publish, OR
- The build is so large that incremental compilation pain demands it (>30 s incremental, ~50k+ LOC), OR
- You have multiple binaries that share a meaningful chunk of code (CLI + GUI + daemon).

A 6,000-line desktop app is not big enough to need a workspace. The boilerplate (`[workspace.dependencies]`, two layers of `Cargo.toml`, redirected paths) costs more than it buys until you cross those thresholds.

### 1.2 Module layout inside `src/`

Flat is fine. `src/foo.rs` over `src/foo/mod.rs + src/foo/bar.rs` until a single file passes ~600 LOC and breaks naturally along a structural seam.

Rules of thumb:

- **One module per responsibility**, named after the domain noun, not the layer (`audio.rs`, `project.rs` — not `models.rs`, `services.rs`). The latter convention is alien to Rust idiom and makes navigation worse.
- **`ui/` subdirectory** for view code if there's a GUI. Keep one file per screen / tab / floating window. View code shouldn't bleed into model modules.
- **No `lib.rs`** unless you're publishing a library. A `[bin]`-only crate uses `main.rs` + sibling `mod.rs` declarations.

### 1.3 Public visibility

`pub` at module level only what other modules genuinely need. Field-level: `pub` for the model types other modules read, otherwise `pub(crate)` or none. Types you serialise to disk (`Profile`, `Track`, `Project`) usually need `pub` on every field for ergonomic field access; that's fine.

`#[doc(hidden)]` on internal-only types you exposed for another reason (test harness, macro support).

---

## 2. Error handling

### 2.1 The library / application split

**Library code** (anything you'd publish on crates.io, or that's reused across binaries): use [`thiserror`](https://docs.rs/thiserror). Define a structured `Error` enum. Each variant carries actionable data.

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("project manifest missing: {path}")]
    ManifestMissing { path: PathBuf },

    #[error("manifest is malformed JSON")]
    ManifestParse(#[from] serde_json::Error),

    #[error("track {name} references missing file {file}")]
    TrackFileMissing { name: String, file: PathBuf },
}
```

Callers can `match` the error and react differently per variant.

**Application code** (the binary's glue, UI handlers, command dispatch): use [`anyhow`](https://docs.rs/anyhow). The vast majority of error sites just need to bubble up to a status bar or modal — the `?` operator + `.context("…")` chain is exactly the right tool.

```rust
use anyhow::{Context, Result};

fn import_zip(path: &Path) -> Result<Project> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    // …
}
```

Don't reach for `Box<dyn Error>` directly. `anyhow::Error` does the same job with a better API and a built-in backtrace.

### 2.2 When NOT to propagate

In the audio callback, NEVER use `?`. The cpal output stream callback is on a real-time thread; an early-return is fine but a panic from `unwrap_or_else(|| panic!(…))` will tear down the audio engine. **Defensive programming with explicit fallbacks** is the rule:

```rust
let g = if c.has_automation {
    samplers[i].sample(t).unwrap_or(c.static_gain_db)
} else {
    c.static_gain_db
};
```

Same for any thread that doesn't have a recovery path. Save your `?` for the request/response code paths.

### 2.3 User-facing error surfacing

`eprintln!` at the binary entrypoint is rarely enough. For a GUI app, plumb errors into a status bar (transient) and a modal (sticky, blocking). TinyBooth's import-result modal is an example: every import — success or fail — pops a modal with the outcome, a copyable summary, and a link to a per-import log file. **Silent failures are the worst UX.** Build a structure where every operation produces a `Report` value the UI knows how to display.

### 2.4 Panics

Panic only on programmer errors that mean continued execution would be wrong (invariant violation, impossible match arm). User-input failures, file-system failures, network failures: never panic. Use `Result`. `unwrap()` and `expect()` are technical debt: each one is a TODO.

The exception is `unsafe { transmute(_).unwrap() }`-style assertions where the safety proof guarantees `Some` — those `expect("safety: …")` calls are documentation, not error paths.

---

## 3. Threading and synchronization

### 3.1 The thread budget

Most desktop apps have:

- **One UI thread** running the event loop (egui's per-frame `update`).
- **0 to N audio threads** owned by the audio framework (cpal spawns one per stream).
- **A handful of short-lived background threads** for one-shot work (HTTP requests, file scans).

That's it. **Do not use a thread pool.** Most desktop work is either UI-bound (single thread mandatory) or short-task (spawn-and-join). A `tokio` runtime is overkill unless you have legitimate concurrent network I/O.

### 3.2 Sharing state across threads

Three patterns, in order of preference:

**Pattern 1: atomics for hot, simple values.**
```rust
struct PlayerState {
    play_pos_frames: AtomicU64,
    global_bypass: AtomicBool,
    master_gain_db_bits: AtomicU32,  // f32 stored as bits via to_bits/from_bits
}
```
Use this for anything the audio thread reads on every sample. `Ordering::Relaxed` is correct for advisory state (UI views of progress, level meters); `Acquire/Release` if you need to publish a payload alongside the flag flip (e.g., generation counter pattern below).

**Pattern 2: `Arc<Mutex<T>>` for cold, structural state.**
```rust
struct VizState {
    samples: parking_lot::Mutex<VecDeque<f32>>,
}
```
Audio thread pushes; UI thread snapshots. Lock window is microseconds. `parking_lot::Mutex` is faster than `std::sync::Mutex` and has no poisoning — strictly better for app code.

**Pattern 3: generation-counter rebuild.**
When the UI thread mutates a heavyweight resource the audio thread needs (a filter chain, a spline sampler), don't push the mutation across the boundary. Instead:

1. UI thread: lock the Mutex, write the new config, unlock, bump an `AtomicU64` generation.
2. Audio thread: at the top of each callback, compare `seen_gen[i]` vs `t.generation.load(Acquire)`; if different, take the lock briefly, clone, build the local resource, update `seen_gen`.

This keeps the audio thread allocation-free 99 % of the time. Only on actual changes does it pay the lock cost.

```rust
// UI thread
*track.profile.lock() = Some(new_profile);
track.generation.fetch_add(1, Release);

// Audio thread (per callback)
let g = track.generation.load(Acquire);
if seen_gen[i] != g {
    let p = track.profile.lock().clone();
    chains[i] = p.map(|p| FilterChain::new(p, sample_rate));
    seen_gen[i] = g;
}
```

### 3.3 What never to put in the audio callback

- **No allocations.** Pre-allocate scratch buffers in the closure's owned state at stream creation. Use `.fill(0.0)` instead of `vec![0.0; n]` to clear them per buffer. `Vec::with_capacity` is a tempting trap: capacity is preserved across `Vec::clear()`, but the next `push` after capacity growth still allocates. Use a fixed-size scratch sized at startup.
- **No `String` formatting.** `format!`, `to_string`, `Display` impls all allocate. Pre-build any text the audio thread emits, or just don't emit text from there at all.
- **No `Mutex` lock holds across more than a handful of instructions.** If you must lock, do it once at the top of the callback for a generation check, not per-sample.
- **No `println!` or `eprintln!`.** Both lock stdout/stderr. Use the cpal error callback for stream-level messages and route them to a UI-thread queue for display.
- **No file I/O.** Pre-load into memory. For longer media, ring-buffer-fed-by-producer-thread is the standard pattern.

### 3.4 Audio buffer sizing

cpal exposes the device's preferred buffer size. You usually want ~256 frames at 48 kHz (~5 ms latency) — small enough to feel responsive, large enough that occasional UI thread hiccups don't underflow the audio. Buffers smaller than 64 frames invite xruns; bigger than 1024 feel sluggish.

For non-real-time processing (export, offline render), you control everything; allocate freely.

---

## 4. GUI patterns (egui-specific)

egui is immediate-mode: every frame your code re-emits the entire UI tree. This is fast, simple, and fights the borrow checker constantly.

### 4.1 The closure-capture borrow problem

```rust
ui.horizontal(|ui| {
    if ui.button("Save").clicked() {
        self.save();           // ← compile error: `self` already borrowed
    }
});
```

The closure passed to `ui.horizontal` borrows `self` because `ui` is a method on `&mut self`. Inside, you can't mutate `self`.

**The fix that's idiomatic in TinyBooth and elsewhere:** capture click intent as a local boolean inside the closure, fire the action after.

```rust
let mut click_save = false;
let mut click_play = false;
ui.horizontal(|ui| {
    if ui.button("Save").clicked() { click_save = true; }
    if ui.button("Play").clicked() { click_play = true; }
});
if click_save { self.save(); }
if click_play { self.play(); }
```

For multi-button rows or modals with complex state, this is the only path that doesn't end in `Rc<RefCell<…>>` or a thousand `.clone()` calls. Embrace it.

### 4.2 Arc-cloning for closure capture

When you need access to a resource inside a closure that *will* outlive the closure (background thread, async callback), `.clone()` an `Arc` first:

```rust
let viz = Arc::clone(&self.viz);
std::thread::spawn(move || {
    viz.push_mono(sample);
});
```

The clone is two atomic operations — cheaper than the alternative of trying to make the closure borrow.

### 4.3 Floating windows for non-modal helpers

Reach for `egui::Window::new(title).open(&mut state.show_foo).collapsible(false).show(ctx, …)` instead of stuffing every feature into a tab. TinyBooth's Admin profile editor, per-track Correction editor, in-app Manual viewer, import-result modal, and conflict-resolution modal are all this pattern. The user can leave them open while doing other work, drag them to a second monitor, or close them with a keypress (egui handles Escape on focused windows).

Avoid centred fullscreen modals unless you genuinely need to block input. They're hostile to the multi-window flow real users expect.

### 4.4 State persistence

eframe has built-in serde-based state persistence (`persistence` feature on eframe). It re-loads on next startup. **Use it for view state**: window size, last-active tab, panel split fractions. **Don't use it for project data** — that's a separate JSON file the user can move around, version-control, share. Mixing the two in the same persistence layer causes confusion when the user manually edits one or moves the binary.

---

## 5. Serialization compatibility

If your binary will outlive its first release (it will), schema migration is a thing.

### 5.1 The `#[serde(default)]` rule

**Every field added after v0.1 gets `#[serde(default)]` or `#[serde(default = "fn_returning_default")]`**. Period. This makes old saved files load on new code without touching them.

```rust
pub struct Track {
    pub id: String,
    pub name: String,
    // …
    #[serde(default)]
    pub correction: Option<Profile>,        // added v0.2
    #[serde(default)]
    pub gain_automation: Option<AutomationLane>,  // added v0.3
}
```

A v0.1 manifest reads cleanly into a v0.3 `Track`; the new fields land at `None`.

**Don't rename fields** without a migration. `#[serde(rename = "old_name")]` works for the read side, but plain renames break v0.1 files silently (they round-trip through the new name and lose the data).

### 5.2 Tagged enums

For tagged sum-types in a manifest:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TrackSource {
    Recorded,
    SunoStem { role: StemRole, original_filename: String },
}
```

Always pick `tag = "…"` over the default-untagged form. Untagged enums are a foot-gun: serde tries each variant in order and uses the first that parses, which gives surprising matches when shapes overlap. Tagged enums are explicit, debuggable, and forward-compatible.

### 5.3 Versioning the schema

Stamp a `version: u32` field on the top-level project type. You can match on it during deserialisation if you ever need to migrate. In practice, with disciplined `#[serde(default)]` you usually never need to bump the version field.

When you DO need a real migration (semantic change to an existing field, not just an addition), do it explicitly with a custom `Deserialize` impl that reads the old shape and produces the new. Test it on a saved sample of pre-migration data.

---

## 6. Dependencies

Rust's ecosystem is fast-moving and crate selection compounds quickly. A few habits:

### 6.1 `default-features = false` by default

Most crates pull in optional features you don't need. Read the docs once, pick the minimum:

```toml
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
eframe  = { version = "0.28", default-features = false, features = ["default_fonts", "glow", "persistence"] }
image   = { version = "0.25", default-features = false, features = ["png"] }
```

This shaves binary size, compile time, and dependency-tree depth measurably. `reqwest` with default features pulls native-tls (which pulls openssl-sys, which needs system libs); with `rustls-tls` it's pure Rust.

### 6.2 Stick to mature crates

For the load-bearing stuff (audio, GUI, serialisation, HTTP), prefer crates that are 1.0+ or have been stable through multiple Rust edition bumps. The diamond pattern (a small fast-moving crate at the centre of a dependency tree) means an upstream rewrite costs you a weekend.

TinyBooth uses: `eframe` (0.28, stable for ~2 years), `cpal` (0.15, stable), `hound` (3.5, hasn't moved in a while because WAV doesn't move), `serde` (1.x, never moves), `reqwest` (0.12, mature), `chrono` (0.4, mature). One outlier: `rustfft` (6.x) but it's API-stable.

### 6.3 Audit before adding

Every new dependency is a security surface, a future-update tax, and a binary-size hit. Ask:

1. Can I write this in 30 lines? (RIFF chunk walker — yes; FFT — no; HTTP/TLS — no.)
2. Does the maintainer respond to issues? (Check the repo's recent commit cadence.)
3. Does it have transitive deps I'd flinch at? (`serde_json` → `itoa` → `ryu` is fine; anything pulling `tokio` for one helper is not.)
4. License compatible? (MIT/Apache-2.0 dual is the safest baseline.)

### 6.4 Don't pin too tightly

`reqwest = "0.12"` accepts `0.12.x`. Avoid `=0.12.3` unless you have a specific reason — it locks you out of patch-level fixes and forces tedious lockfile maintenance.

Conversely, `Cargo.lock` should be checked in for binaries (not for libraries). Reproducible CI builds need it.

---

## 7. Build & distribution

### 7.1 Version derivation

Don't hand-edit a version constant. Derive it from git in `build.rs`:

```rust
fn main() {
    let version = std::process::Command::new("git")
        .args(["describe", "--tags", "--match", "v*", "--abbrev=0"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .map(|s| s.trim().trim_start_matches('v').to_string())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    println!("cargo:rustc-env=APP_VERSION={version}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}
```

The binary now reads `env!("APP_VERSION")`. Tags drive everything; Cargo.toml's version is a fallback for non-git builds.

### 7.2 Embedding resources

`include_bytes!` and `include_str!` are your friends. Documentation, icons, fonts, default config files all belong embedded into the binary on the first launch — no install-path resolution, no broken symlinks, no version drift between docs and code.

```rust
const VIEWPORT_ICON_PNG: &[u8] = include_bytes!("../assets/icon_viewport.png");
const MANUAL_INTRO: &str = include_str!("../docs/manual/00-index.md");
```

### 7.3 Windows specifics

For a Windows GUI binary:

- `#![windows_subsystem = "windows"]` at the crate root prevents the console window from popping up on launch.
- `winres` crate in `build.rs` embeds a `.ico` and a Windows version resource block (visible in File Explorer's Properties → Details).
- `cargo-wix` produces an MSI from a `wix/main.wxs` template. Worth the setup; users expect MSIs on Windows.
- Code-signing the MSI is a separate concern (EV cert, ~$300/yr); do it only when you actually have users complaining about SmartScreen warnings.

### 7.4 CI (GitHub Actions)

Tag-driven release is the cleanest pattern:

1. `git tag vX.Y.Z && git push origin vX.Y.Z`
2. Workflow trigger: `on: push: tags: ['v*']`
3. Build → package → upload artefacts → create release with auto-generated notes.

Don't bake secrets into the workflow file. `${{ secrets.GITHUB_TOKEN }}` is automatic for releases on the same repo. For external services (sentry, signing, S3), use repo Secrets.

Pin actions to a major version (`actions/checkout@v5`) — patch updates flow automatically, breaking changes are explicit. Pin to a SHA only if you're paranoid about supply chain.

### 7.5 Self-update

A binary the user installed once should be able to update itself. Pattern:

1. On startup, query `https://api.github.com/repos/OWNER/REPO/releases/latest`. Public unauthenticated endpoint, ~60 req/hr per IP.
2. Compare `tag_name` against `APP_VERSION`. Use 4-part semver to allow `.4` style hot-fixes.
3. If newer, surface a clickable banner. On click: download the `.msi` to temp.
4. Launch the installer with elevation and exit. Windows takes over.

```powershell
Start-Process msiexec -ArgumentList '/i "{path}" /passive /norestart' -Verb RunAs
```

Don't auto-install without user click. People hate that.

---

## 8. Logging and diagnostics

### 8.1 The two channels

**stderr for ambient noise** — anything you'd grep for during development. Users running from a terminal see it; users double-clicking the icon don't. Free, zero overhead.

**File-based logs for user-shareable diagnostics** — when an operation fails in a way the user might want to report, write a structured log to `%APPDATA%\AppName\logs\<operation>-<timestamp>.log`. Surface the path in the failure dialog with an "Open log" button.

TinyBooth's Suno-import logs are a worked example: every entry's KEEP/SKIP decision is logged with its rationale, plus a counter summary at the end. When a user says "the import does nothing", the log answers "it skipped 27 files because they were all Tempo-Locked variants" without anyone needing to attach a debugger.

### 8.2 What to log

- **Boundaries** — every ingest, every export, every external call.
- **Decisions** — why a code path was taken (especially when filtering/skipping).
- **Counters** — at the end of any loop processing user data.
- **Errors with context** — `anyhow::Context` chains read like log entries by themselves.

### 8.3 What NOT to log

- User-identifiable content (file contents, names of personal files) by default. If the user shares the log, they shouldn't accidentally leak their workflow.
- Anything in the audio callback. File logging from a real-time thread is the worst pattern.
- Telemetry phoning home. Logs should be local-first; sharing should be an explicit copy/paste by the user.

---

## 9. Testing

### 9.1 The testing budget for an app

Be honest about what testing buys you for a desktop GUI. The integration test that actually drives an egui app is brittle, slow, and rewards you with maybe 5% of the bugs you'll ship.

What pays back:

- **Pure-function unit tests** for any math, parsing, or state machine. (TinyBooth's spline-padding logic, RIFF chunk walker, file-name-to-StemRole matcher, version comparator are all easy to test.)
- **Round-trip tests** for serialisation. Save a manifest, read it back, assert equality. Catches schema changes that break old files.
- **Property tests** via [`proptest`](https://docs.rs/proptest) on parsers and schedulers. Goes well past what you'd hand-write.
- **Snapshot tests** via [`insta`](https://docs.rs/insta) on any text output (reports, log lines, exported manifests). Cheap, catches accidental format drift.

What rarely pays back at our scale:

- **End-to-end UI tests.** egui doesn't have an automatable harness. Driving the OS-level mouse via WinAPI to click your own buttons is a tar pit.
- **Audio render-and-compare**. Hard to make deterministic across cpal versions / OS versions / device buffer sizes.

If you skip these, ship a manual smoke-test checklist and hold yourself to running it before tagging a release. Even a 10-step checklist catches more shipping bugs than a flaky integration test would.

### 9.2 Test data on disk

Put fixture files under `tests/fixtures/` and load them via relative paths. For Windows-line-ending concerns, configure `core.autocrlf = false` in the repo's `.gitattributes` for binary files.

---

## 10. Performance

### 10.1 Don't optimise without measuring

Most "obvious" performance fixes don't move the needle. Use [`cargo flamegraph`](https://github.com/flamegraph-rs/flamegraph) (or `perf` directly on Linux) to find the actual hot path, then optimise specifically that.

For micro-benchmarks, [`criterion`](https://docs.rs/criterion) is the standard. Add it under `[dev-dependencies]`, write a `benches/foo.rs`, run `cargo bench`. Compares against a saved baseline.

### 10.2 Allocation discipline

The single biggest performance lever in a Rust app is "don't allocate in the hot path". The TinyBooth audio callback's Phase B refactor (v0.3.4) eliminated per-callback `Vec` allocs and dropped per-sample atomic loads ~250×. Same pattern applies anywhere a function runs at high frequency.

Patterns:

- **Pre-allocate scratch buffers** in long-lived state, reuse across iterations. `Vec::clear()` keeps the capacity.
- **Use `.fill(default)`** instead of `iter_mut().for_each(|x| *x = default)`.
- **Avoid `format!`** in hot paths. Pre-format once and store; or use `write!(buf, …)` into a reused `String`.
- **Cache atomically-loaded values** at the top of a tight loop instead of re-loading per iteration.

### 10.3 `Box<dyn Trait>` vs generics

Generic code monomorphises — fast at runtime, slower to compile, larger binary. Trait-object code has dynamic dispatch — slower at runtime (vtable indirection), faster to compile, smaller binary.

For audio callbacks: generics. For UI callbacks: doesn't matter; pick whichever reads better.

### 10.4 Profile-guided optimisation (PGO)

PGO can win 5–15 % on real workloads. It's also a build-pipeline complication (instrumented build → train run → optimised build). Not worth it for most apps; revisit if you're shipping something CPU-bound where the workload is well-defined (compilers, codecs, simulation engines).

---

## 11. Security and safety

### 11.1 `unsafe` is rarely justified

For application code: zero `unsafe` blocks should be your goal. The standard library and well-vetted crates wrap whatever low-level primitive you need.

If you find yourself reaching for `unsafe`, ask:

1. Can `bytemuck` / `zerocopy` / a pure-Rust alternative do this safely?
2. Am I optimising prematurely? (FFI is the only legitimate frequent reason for `unsafe`.)
3. Do I have a written safety proof in a comment? If not, you don't understand the invariants well enough to use `unsafe`.

### 11.2 Supply-chain awareness

Every dependency is code that runs in your binary. Habits:

- Use [`cargo-audit`](https://docs.rs/cargo-audit) or [`cargo-deny`](https://embarkstudios.github.io/cargo-deny/) in CI. RUSTSEC advisories surface as build failures.
- Pin `Cargo.lock` for binaries. Reproducible builds, deterministic deploys.
- Periodically run `cargo update --dry-run` and review what's about to move. Don't blindly run `cargo update` before a release — let one PR review it.
- Watch for typo-squatted crate names (`tokio_macros` vs `tokio-macros`, etc.).

### 11.3 Input validation

Anything user-supplied (file paths, manifest content, network responses) is hostile until proven otherwise. `serde_json::from_str(&untrusted)` with a denial-of-service budget — for a pathological 1 GB JSON file, what happens? Default behaviour: it allocates 1 GB. Use `serde_json::from_reader` against a `Take<Reader>` with a length cap if the source is untrusted.

Path traversal: `Path::join` doesn't sanitise. If you're constructing paths from external data (zip entries — see TinyBooth's `enclosed_name` use), explicitly check for `..` components.

---

## 12. The shipping checklist

Before tagging a release:

1. `cargo fmt --check` clean.
2. `cargo clippy --release -- -D warnings` clean.
3. `cargo test` green.
4. `cargo audit` clean.
5. Manual smoke test: open the app, do the three principal flows.
6. CHANGELOG entry written (if you maintain one).
7. Cargo.toml version matches the tag you're about to push.
8. Git working tree clean and pushed.
9. Tag → CI → release → in-app updater surfaces it.

If any step fails, fix and re-run. Don't tag at half-speed.

---

## 13. The patterns that consistently failed

Negative examples worth internalising — things that look right and bite later:

- **Async by default.** `tokio` for a desktop app's HTTP version-check is overkill. A blocking `reqwest::blocking::Client` on a one-shot thread is 30 lines lighter, doesn't drag a runtime in, and is easier to reason about.
- **Excessive trait abstraction.** "What if we want to swap audio backends one day?" — you don't. cpal abstracts that already. Defining `trait AudioInput { fn build(...) -> Box<dyn AudioInput>; }` to wrap cpal is busywork that buys nothing and costs hundreds of lines.
- **Premature config.** `dirs` + `serde_json` for a 4-field config is right. A 200-line `config-rs` integration with environment-variable overrides and a TOML-vs-YAML toggle is wrong for a desktop app.
- **Trying to use `Result<T, E>` for control flow.** If a function "returns Err to mean the user clicked Cancel", that's an `Option` or a domain-specific enum, not a `Result`. Save `Result` for failures.
- **Building everything as `pub`.** Module visibility is the single best documentation tool Rust gives you. `pub` exposes contract; everything else is implementation.
- **Wrapping every error type.** If your `fn` chain calls into 5 crates, you don't need `MyError::FromCpal(cpal::Error)`, `MyError::FromHound(hound::Error)`, etc. Use `anyhow::Result` and let `?` do the work. The structured error enum is the correct shape only if the *caller* needs to discriminate.

---

## 14. Closing reads

Books / docs that pay back:

- The Rust Book — keep returning to it. The traits chapter and the unsafe chapter especially.
- The async book — even if you don't use async, read it. It's the clearest explanation of what `Pin`, `Send + 'static`, and lifetime bounds actually mean.
- The cargo-deny docs — a half-hour read that pays back forever.
- The egui demo source code (https://github.com/emilk/egui) — best place to see real-world immediate-mode patterns.
- The cpal and hound source — both are small enough to read end-to-end. Real Rust audio code, no magic.

Habit-of-mind: when something feels harder than it should, the issue is almost always that you're fighting Rust's model. Step back, identify the ownership story, redesign so the language is on your side. Once you have that habit, ergonomic Rust stops feeling like a struggle and starts feeling like the language is doing your bookkeeping for you.
