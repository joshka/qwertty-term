//! Semantic prompt (OSC 133) state owned by `Screen`.
//!
//! The `SemanticPrompt` container and `seen` optimization are Screen's own; the
//! `Click` / `ClickEvents` / `PromptKind` / `Redraw` enums originate in
//! `src/terminal/osc/parsers/semantic_prompt.zig`.
//!
//! TODO(chunk:osc): replace these local placeholders with re-exports from the
//! ported OSC parser module once the `osc` sibling chunk lands. They are defined
//! narrowly here (values only, no parsing) so Screen's resize/prompt plumbing
//! and `cursorSetSemanticContent` compile without depending on the OSC chunk.

/// OSC 133 click-events option. Port of `osc.semantic_prompt.ClickEvents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickEvents {
    Absolute,
    Relative,
}

/// OSC 133 click-handling option. Port of `osc.semantic_prompt.Click`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Line,
    Multiple,
    ConservativeVertical,
    SmartVertical,
}

/// The kind of prompt line. Port of `osc.semantic_prompt.PromptKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Initial,
    Right,
    Continuation,
    Secondary,
}

/// Whether/how the shell supports prompt redraw on resize. Port of
/// `osc.semantic_prompt.Redraw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Redraw {
    /// Shell redraws the full prompt and all continuations.
    True,
    /// Shell does NOT redraw — Ghostty clears nothing on resize.
    #[default]
    False,
    /// Shell redraws only the LAST prompt line (e.g. Bash).
    Last,
}

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
