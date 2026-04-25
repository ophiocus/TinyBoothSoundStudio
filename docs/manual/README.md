# TinyBooth Sound Studio — Manual

The same chapters served from the in-app Help → Manual… window.

The files in this folder are the **single source of truth**: they render here on GitHub for browsing on the web, and they are embedded into the binary at compile time via `include_str!` ([`src/manual.rs`](../../src/manual.rs)) so the running app shows byte-identical content. Edit any chapter and it updates both surfaces on the next build.

## Welcome

- [Welcome](00-index.md) — what TinyBooth is, philosophy in one sentence, where things live on disk.
- [Getting started](01-getting-started.md) — first-run walkthrough; mic to saved take in five minutes.

## Reference

- [Recording](02-recording.md) — the Record tab in detail.
- [Recording tones](03-recording-tones.md) — the Profile / FilterChain system, presets.
- [Editing profiles (Admin)](04-admin.md) — every parameter the Admin window exposes.
- [Projects](05-projects.md) — the `.tinybooth` project format and the Project tab.
- [Export](06-export.md) — formats, mixdown, ffmpeg discovery.
- [Importing Suno stems](07-suno-import.md) — folder + zip ingestion, role tagging.
- [Self-update](08-self-update.md) — the in-app updater wired to GitHub Releases.
- [Mix tab — remastering](10-mix.md) — multitrack playback, per-track correction, A/B bypass.
- [Using this manual](09-using-this-manual.md) — how the Help window itself works.

## Appendix

- [Troubleshooting](appendix-a-troubleshooting.md)
- [File formats](appendix-b-file-formats.md) — `project.tinybooth`, `profiles.json`, `config.json`, track WAV specs, Suno-stem filename mapping.

## Editing

If you spot a typo or want to extend a chapter, the pages are plain Markdown. Open a PR; the next tagged release will ship the updated text both in this folder and inside the in-app manual. No separate documentation pipeline.
