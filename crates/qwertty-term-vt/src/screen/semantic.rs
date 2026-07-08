//! Semantic prompt (OSC 133) state owned by `Screen`.
//!
//! The `SemanticPrompt` container and its `seen` optimization are Screen's own.
//! The `Click` / `ClickEvents` / `PromptKind` / `Redraw` vocabulary is the real
//! parsed OSC 133 type set, re-exported here from
//! [`crate::osc`] (the OSC chunk has landed), so
//! Screen and Terminal share exactly one definition rather than the former
//! local placeholder copy.

pub use crate::osc::{Click, ClickEvents, PromptKind, Redraw};

/// How click handling in a prompt is configured. Port of
/// `SemanticPrompt.SemanticClick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SemanticClick {
    #[default]
    None,
    ClickEvents(ClickEvents),
    Cl(Click),
}

/// Semantic prompt (OSC 133) state. Port of `Screen.SemanticPrompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticPrompt {
    /// Flipped true the first time any `prompt` semantic content is set. Used to
    /// optimize away semantic content operations if we've never seen one.
    pub seen: bool,
    /// The most recent `cl` / `click_events` OSC 133 option.
    pub click: SemanticClick,
}

impl Default for SemanticPrompt {
    /// Port of `SemanticPrompt.disabled`.
    fn default() -> Self {
        SemanticPrompt {
            seen: false,
            click: SemanticClick::None,
        }
    }
}
