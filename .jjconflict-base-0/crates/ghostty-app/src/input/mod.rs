//! Input path: macOS keycode mapping, key translation/encoding, and IME preedit
//! state. Lifted from the R5 spike (`spikes/appkit-input/`) into production form.
//!
//! - [`keymap`]: macOS native keycode → `ghostty_input::key::Key`.
//! - [`translate`]: raw `NSEvent` data → `KeyEvent` → encoded PTY bytes, plus
//!   option-as-alt handling.
//! - [`preedit`]: the `NSTextInputClient` marked-text state machine.
//!
//! All three are AppKit-free and unit-tested here; the macOS `NSView` that feeds
//! them lives in [`crate::view`].

pub mod keymap;
pub mod mouse;
pub mod preedit;
pub mod translate;
