# TBSS-FR-0016 — Keyboard focus & keymap interface

| | |
|---|---|
| **ID** | TBSS-FR-0016 |
| **Title** | One standardized keyboard interface: focus precedence + declarative keymap |
| **Status** | ✅ Implemented (core) |
| **Filed / landed** | 2026-08-24, v0.4.82 |
| **Requested by** | Carlos — reacting to FR-0014's "first UI in the app to need a real focus model" risk: "this is good. Standardize as a general interface" |
| **Feeds** | FR-0014 (tracker grid is the first Editor-scope tenant); the audit's "keyboard shortcuts: only F1 exists" completeness gap |

## Executive summary

Before this, the app had exactly one shortcut (F1, an ad-hoc check) and
every future keyboard consumer — above all the FR-0014 tracker's
FastTracker-style pattern entry — would have invented its own handling
and fought the others for keys. `src/keymap.rs` standardizes it: a
**fixed focus-precedence order** decides who owns the keyboard each
frame, and a **declarative binding registry** (shortcut + action + scope
+ description) drives dispatch — the same table a future shortcut-help
overlay and user rebinding will read.

## The precedence order (the interface's core rule)

1. **Text entry** — a focused text field owns everything; no app binding
   fires (`ctx.wants_keyboard_input()`).
2. **Modal scope** — any open modal restricts dispatch to Modal-scope
   bindings (Esc → close-topmost).
3. **Editor scope** — a widget doing its own key handling (tracker
   grid) claims the frame; only Global bindings flagged
   `override_editor` (Ctrl+S) may still fire.
4. **Global scope** — everything else.

Resolution is **pure** (`Keymap::resolve` takes a `pressed` closure), so
the precedence rules are unit-tested without an egui context — including
the property that a disallowed binding is *never probed*, so the egui
adapter's `consume_shortcut` can't eat keys belonging to a text field or
an editor. Six tests pin the contract.

## Editor-widget contract (what FR-0014 implements)

- **Claim**: request egui focus on click; while `response.has_focus()`,
  set `app.keyboard_editor_active = true` every frame (the dispatcher
  clears it after reading — release is implicit on focus loss).
- **Consume**: read keys directly from `ctx.input(...)`; nested text
  fields fall back to precedence rule 1 automatically.
- **Respect overrides**: don't bind chords the registry marks
  `override_editor` (today: Ctrl+S, F1).

## Bindings shipped (v0.4.82)

| Keys | Action | Scope | Overrides editor |
|---|---|---|---|
| `Space` | Play/pause the current audible thing (recording preview first, else mixer) | Global | no |
| `Ctrl+S` | Save project | Global | **yes** |
| `F1` | Toggle manual (migrated from the ad-hoc check) | Global | **yes** |
| `Esc` | Close topmost modal | Modal | — |

Esc close order: import result → generator params → crossfade bounce
flow → correction editor → audio devices → telemetry settings → health →
trim → admin. The **migration prompt is deliberately not Esc-closable**
(its two choices differ materially — an explicit click is required), and
the manual doesn't count as a modal (it's a reference window; counting
it would kill its own F1 toggle).

## Deferred

Shortcut-help overlay rendered from the registry (`Keymap::bindings()` +
`description` are already in place for it); user rebinding persisted in
config; per-tab scope (transport keys that differ by tab); Delete on
list selections.
