//! TBSS-FR-0016 — the app-wide keyboard focus & keymap interface.
//!
//! One place decides who owns the keyboard each frame, in a fixed
//! precedence order:
//!
//! 1. **Text entry** — any focused egui text field owns everything
//!    (`ctx.wants_keyboard_input()`); no app binding fires.
//! 2. **Modal scope** — while any modal window/dialog is open, only
//!    `Scope::Modal` bindings fire (Esc closes the topmost).
//! 3. **Editor scope** — a widget that does its own key handling (the
//!    FR-0014 tracker pattern grid is the intended first tenant) sets
//!    [`crate::app::TinyBoothApp::keyboard_editor_active`] every frame it
//!    holds focus and consumes raw keys itself. While active, only
//!    global bindings marked `override_editor` fire (Ctrl+S must always
//!    save; Space must NOT start the mixer under a grid that uses Space).
//! 4. **Global scope** — everything else.
//!
//! Bindings live in one declarative table ([`Keymap::default_bindings`])
//! with descriptions, so a future shortcut-help overlay and user
//! rebinding read the same registry the dispatcher does. The resolution
//! logic is pure — `resolve` takes a `pressed` closure, so tests drive
//! it without an egui context — and the egui adapter consumes matched
//! shortcuts so they never double-fire into widgets below.
//!
//! Contract for editor widgets (write this once, every editor obeys it):
//! * claim: request egui focus on click; while `response.has_focus()`,
//!   set `app.keyboard_editor_active = true` (it resets every frame).
//! * consume: read keys via `ctx.input(...)` yourself; text fields
//!   inside the editor drop you back to rule 1 automatically.
//! * release: losing egui focus (click elsewhere, Esc) releases the
//!   scope implicitly — no unregister call exists or is needed.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

/// Everything a binding can do. Kept as data so the registry, the
/// dispatcher, and (later) a help overlay agree by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Space — toggle the current audible thing: stops an in-listing
    /// recording preview if one is playing, else play/pauses the mixer.
    TogglePlayback,
    /// Ctrl+S — save the open project (folder or `.tib`).
    SaveProject,
    /// F1 — toggle the built-in manual.
    ToggleManual,
    /// Esc — close the topmost open modal.
    CloseTopModal,
}

/// Which focus layer a binding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Modal,
}

pub struct Binding {
    pub shortcut: KeyboardShortcut,
    pub action: Action,
    pub scope: Scope,
    /// Fires even while an editor widget owns the keyboard. Reserve for
    /// bindings that must never be shadowed (save). Chord-modified keys
    /// are good candidates; bare keys almost never are.
    pub override_editor: bool,
    /// Human-readable line for the future shortcut-help overlay.
    /// (Per-item allow, not module-level — the audit's rule: blanket
    /// allows mask real corpses.)
    #[allow(dead_code)]
    pub description: &'static str,
}

/// Snapshot of who could own the keyboard this frame. Built by the app
/// from egui + its own state; consumed by [`Keymap::resolve`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FocusState {
    /// A text field has egui keyboard focus.
    pub text_editing: bool,
    /// At least one modal window/dialog is open.
    pub modal_open: bool,
    /// An editor widget (tracker grid, …) claimed the keyboard.
    pub editor_active: bool,
}

pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: Self::default_bindings(),
        }
    }
}

impl Keymap {
    fn default_bindings() -> Vec<Binding> {
        vec![
            Binding {
                shortcut: KeyboardShortcut::new(Modifiers::NONE, Key::Space),
                action: Action::TogglePlayback,
                scope: Scope::Global,
                override_editor: false,
                description: "Play / pause (mixer or recording preview)",
            },
            Binding {
                shortcut: KeyboardShortcut::new(Modifiers::CTRL, Key::S),
                action: Action::SaveProject,
                scope: Scope::Global,
                override_editor: true,
                description: "Save project",
            },
            Binding {
                shortcut: KeyboardShortcut::new(Modifiers::NONE, Key::F1),
                action: Action::ToggleManual,
                scope: Scope::Global,
                override_editor: true,
                description: "Toggle manual",
            },
            Binding {
                shortcut: KeyboardShortcut::new(Modifiers::NONE, Key::Escape),
                action: Action::CloseTopModal,
                scope: Scope::Modal,
                override_editor: false,
                description: "Close the open dialog",
            },
        ]
    }

    /// All bindings, for a help overlay / rebinding UI.
    #[allow(dead_code)] // consumer is the FR-0016 help overlay follow-up
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Whether a binding may fire under the given focus state — the
    /// precedence rules from the module doc, as one pure predicate.
    fn fires(&self, b: &Binding, focus: &FocusState) -> bool {
        if focus.text_editing {
            return false;
        }
        if focus.modal_open {
            return b.scope == Scope::Modal;
        }
        if b.scope == Scope::Modal {
            return false; // nothing modal to act on
        }
        if focus.editor_active {
            return b.override_editor;
        }
        true
    }

    /// Resolve this frame's actions. `pressed` reports (and should
    /// consume, in the egui adapter) a matched shortcut; it is only
    /// called for bindings the focus rules allow, so consumption can't
    /// eat keys that belong to a text field or an editor widget.
    pub fn resolve(
        &self,
        focus: &FocusState,
        mut pressed: impl FnMut(&KeyboardShortcut) -> bool,
    ) -> Vec<Action> {
        self.bindings
            .iter()
            .filter(|b| self.fires(b, focus))
            .filter(|b| pressed(&b.shortcut))
            .map(|b| b.action)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(target: KeyboardShortcut) -> impl FnMut(&KeyboardShortcut) -> bool {
        move |s| *s == target
    }
    const SPACE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Space);
    const CTRL_S: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
    const ESC: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Escape);

    #[test]
    fn global_bindings_fire_with_no_special_focus() {
        let km = Keymap::default();
        let acts = km.resolve(&FocusState::default(), press(SPACE));
        assert_eq!(acts, vec![Action::TogglePlayback]);
    }

    #[test]
    fn text_focus_silences_everything() {
        let km = Keymap::default();
        let focus = FocusState {
            text_editing: true,
            ..Default::default()
        };
        // Even the strongest binding must not fire into a text field.
        assert!(km.resolve(&focus, press(CTRL_S)).is_empty());
        assert!(km.resolve(&focus, press(SPACE)).is_empty());
    }

    #[test]
    fn modal_focus_allows_only_modal_bindings() {
        let km = Keymap::default();
        let focus = FocusState {
            modal_open: true,
            ..Default::default()
        };
        assert!(km.resolve(&focus, press(SPACE)).is_empty());
        assert_eq!(km.resolve(&focus, press(ESC)), vec![Action::CloseTopModal]);
    }

    #[test]
    fn esc_is_inert_when_no_modal_is_open() {
        let km = Keymap::default();
        assert!(km.resolve(&FocusState::default(), press(ESC)).is_empty());
    }

    /// The contract the FR-0014 tracker grid depends on: while an editor
    /// owns the keyboard, bare keys (Space) are its alone, but chorded
    /// must-work bindings (Ctrl+S) still reach the app.
    #[test]
    fn editor_scope_shadows_bare_keys_but_not_overrides() {
        let km = Keymap::default();
        let focus = FocusState {
            editor_active: true,
            ..Default::default()
        };
        assert!(km.resolve(&focus, press(SPACE)).is_empty());
        assert_eq!(km.resolve(&focus, press(CTRL_S)), vec![Action::SaveProject]);
    }

    /// The pressed-closure is never even consulted for a disallowed
    /// binding — the adapter's consume can't eat an editor's keys.
    #[test]
    fn disallowed_bindings_are_never_probed() {
        let km = Keymap::default();
        let focus = FocusState {
            editor_active: true,
            ..Default::default()
        };
        let mut probed = Vec::new();
        let _ = km.resolve(&focus, |s| {
            probed.push(*s);
            false
        });
        assert!(
            !probed.contains(&SPACE),
            "Space was probed (and would have been consumed) while an editor owned it"
        );
        assert!(probed.contains(&CTRL_S));
    }
}
