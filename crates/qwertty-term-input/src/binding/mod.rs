//! Keybind system — port of `input/Binding.zig` (upstream `2da015cd6`).
//!
//! This is the trigger/action model and its parse grammar: a `Trigger` (key +
//! mods) bound to an `Action`, with `Flags` prefixes (`unconsumed:`, `all:`,
//! `global:`, `performable:`). The runtime `Set` (storage, folded lookup,
//! reverse map, sequences/leaders) builds on these types and lands in a
//! follow-up slice; see `docs/analysis/keybinds.md` for the full study and the
//! ordered port plan.
//!
//! Design note: this collapses the app's four bespoke key tables
//! (`tabkeys`/`splitkeys`/`searchkeys`/`keybind` `text:`) into one system.
//! Deleting those tables is the proof the port is real.

pub mod action;
pub mod flags;
pub mod parser;
pub mod trigger;

pub use action::Action;
pub use flags::Flags;
pub use parser::{Binding, ParseItem, Parser};
pub use trigger::{Trigger, TriggerKey};

/// Errors from parsing a keybind. Port of `Binding.Error`
/// (`error{InvalidFormat, InvalidAction}`, Binding.zig:25-28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// The trigger or overall binding syntax was malformed.
    InvalidFormat,
    /// The action name was unknown or the action is not settable from config.
    InvalidAction,
}
