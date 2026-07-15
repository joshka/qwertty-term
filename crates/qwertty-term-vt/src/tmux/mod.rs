//! tmux protocol support (control mode). Port of `terminal/tmux/`
//! (Ghostty `2da015cd6`). See ADR 004.
//!
//! This crate owns the *pure* protocol parsers; the native viewer that maps
//! notifications to surfaces lives in the app/termio layer. Slice 1 (this
//! module set) ports the control-mode parser; `layout` and `output` follow
//! (ADR 004 slices 2–3).

pub mod control;

pub use control::{BufferOverflow, ControlParser, Notification};
