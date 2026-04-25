# Using this manual

The manual you're reading is built into the TinyBooth binary. There is no separate documentation download, no install path resolution, no chance of a broken link after a partial upgrade. Updates ship with each release.

## Opening it

Three ways:

- **Help → Manual…** in the top menu.
- **F1** anywhere in the app.
- Clicking the manual icon, where one is offered (e.g. some future "?" buttons next to feature areas).

## Layout

- **Left pane** — table of contents, grouped by category (Welcome / Reference / Appendix). Click any title to jump to that page.
- **Right pane** — the rendered Markdown of the selected page. Scroll with the mouse wheel or trackpad.

The window is **floating and non-modal**. You can resize it, move it out of the way, or leave it open while you record — recording, visualisation, and the self-update poll all keep running in the background.

## State

- The window remembers which page was last viewed within a single app session. Closing and reopening the manual returns you to the same page.
- Across app restarts, the manual defaults to the **Welcome** page.
- The manual's open/closed state is **not** persisted in `config.json` — this is a transient view, not a setting.

## Why no search

Out of scope for v1. The TOC is short enough (under 15 entries) that linear scanning is faster than typing a query. If the manual outgrows that, search will land — until then, fewer moving parts.

## Why no cross-page links inside the markdown

Same reason — handling click-on-a-link to jump to another page would mean intercepting URL handling in the renderer. Right now, plain-text references like *(see Recording tones)* do the job. If the page count grows, this is a reasonable next addition.

## Where the source lives

Each chapter is a Markdown file under `docs/manual/` in the source repository. They're plain text. The `cargo build` step embeds each one via Rust's `include_str!`, so what you see here is byte-identical to what's on disk in the repo at the time the binary was built.

If you spot a typo or unclear paragraph, the file path is part of the chapter heading in the repo — open a PR or drop an issue at `https://github.com/ophiocus/TinyBoothSoundStudio`.
