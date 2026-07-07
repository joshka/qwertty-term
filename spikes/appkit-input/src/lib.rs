//! R5 de-risk spike: macOS text input through an AppKit `NSTextInputClient`
//! `NSView` into `ghostty-input`'s encoder.
//!
//! Goal (per `docs/plans/m3-first-pixels.md` R5): prove dead keys, IME
//! composition, and modifier fidelity — including `macos-option-as-alt` — flow
//! correctly from AppKit into `ghostty_input::key_encode::encode`, and settle
//! whether R5's window host should be raw AppKit or winit 0.30. See
//! `docs/analysis/appkit-input.md` for the written analysis + recommendation.
//!
//! Structure:
//! - [`keymap`] — macOS native keycode -> `ghostty_input::key::Key`.
//! - [`translate`] — AppKit-free core: raw event -> `KeyEvent` -> encoded bytes,
//!   incl. `macos-option-as-alt`. Fully unit-tested without a window.
//! - [`preedit`] — the IME marked-text state machine (setMarkedText /
//!   unmarkText / insertText). Unit-tested without an input context.
//! - `view` (macOS only) — the `NSView` + `NSTextInputClient` shell wiring the
//!   above to real `keyDown:` / `interpretKeyEvents`.

pub mod keymap;
pub mod preedit;
pub mod translate;

#[cfg(target_os = "macos")]
pub mod view;
