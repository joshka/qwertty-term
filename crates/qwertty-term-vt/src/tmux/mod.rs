//! tmux protocol support (control mode). Port of `terminal/tmux/`
//! (Ghostty `2da015cd6`). See ADR 004.
//!
//! This crate owns the *pure* protocol parsers; the native viewer that maps
//! notifications to surfaces lives in the app/termio layer. Slices so far:
//! `control` (notification parser), `layout` (window-layout parser); `output`
//! follows (ADR 004 slice 3).

pub mod control;
pub mod layout;

pub use control::{BufferOverflow, ControlParser, Notification};
pub use layout::{Checksum, Content, Layout, ParseError};
