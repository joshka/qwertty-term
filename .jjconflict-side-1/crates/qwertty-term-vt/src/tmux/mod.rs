//! tmux protocol support (control mode). Port of `terminal/tmux/`
//! (Ghostty `2da015cd6`). See ADR 004.
//!
//! This crate owns the *pure* protocol parsers; the native viewer that maps
//! notifications to surfaces lives in the app/termio layer. Slices so far:
//! `control` (notification parser), `layout` (window-layout parser), `output`
//! (format-variable parse/encode). The DCS `1000p` seam wiring — feeding
//! control-mode bytes into [`ControlParser`] and surfacing [`Notification`]s on
//! the engine's event queue (`stream::TerminalHandler::take_tmux_notifications`)
//! — landed as ADR 004 slice 4 (`crate::dcs`). The native Viewer is slice 5
//! (app-tails).

pub mod control;
pub mod layout;
pub mod output;

pub use control::{BufferOverflow, ControlParser, Notification};
pub use layout::{Checksum, Content, Layout};
pub use output::{Value, ValueKind, Variable};

// NOTE: both `layout` and `output` define a `ParseError`; neither is re-exported
// here to avoid a name collision — use `layout::ParseError` / `output::ParseError`.
// (`layout::ParseError` was exported as `tmux::ParseError` in slice 2; that alias
// is intentionally dropped now that a second `ParseError` exists.)
