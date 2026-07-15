//! Platform-independent preedit (IME marked-text) state.
//!
//! Lifted verbatim from the R5 spike (`spikes/appkit-input/src/preedit.rs`); it
//! was already platform-independent and production-shaped. Upstream stores
//! marked text as an `NSMutableAttributedString` on the surface and syncs it via
//! `ghostty_surface_preedit` (`SurfaceView_AppKit.swift::syncPreedit`). The
//! state machine that matters for encoding is small and AppKit-free, so it lives
//! here and is unit-tested directly — which is what lets the IME path be
//! verified without a real input context.
//!
//! Transitions, matching the `NSTextInputClient` methods:
//!
//! - `setMarkedText(s)`  → [`Preedit::set_marked`] (enter/continue composing)
//! - `unmarkText()`      → [`Preedit::unmark`]     (composing cleared)
//! - `insertText(s)`     → [`Preedit::commit`]     (composing committed to text)
//!
//! The current marked string ([`Preedit::marked_text`]) is what a future render
//! chunk will draw as the inline preedit overlay; R5 stores it but does not yet
//! render it (documented deferral).

/// The IME marked-text / preedit state for a surface.
#[derive(Debug, Default, Clone)]
pub struct Preedit {
    /// The current marked (preedit) string. Empty when not composing.
    marked: String,
    /// Text committed via `insertText` while a key event was being processed
    /// (upstream `keyTextAccumulator`), drained after `interpretKeyEvents`.
    committed: Vec<String>,
    /// Whether we're inside a keyDown processing window (upstream:
    /// `keyTextAccumulator != nil`). While true, `insertText` accumulates.
    in_key_event: bool,
}

impl Preedit {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if there is active preedit text (`hasMarkedText`).
    pub fn is_composing(&self) -> bool {
        !self.marked.is_empty()
    }

    /// The current preedit string. Empty means "clear preedit".
    pub fn marked_text(&self) -> &str {
        &self.marked
    }

    /// Begin processing a keyDown (upstream: `keyTextAccumulator = []`).
    pub fn begin_key_event(&mut self) {
        self.in_key_event = true;
        self.committed.clear();
    }

    /// End keyDown processing and return any text the IME committed during it.
    pub fn end_key_event(&mut self) -> Vec<String> {
        self.in_key_event = false;
        std::mem::take(&mut self.committed)
    }

    /// `setMarkedText:` — set/replace the composing string.
    pub fn set_marked(&mut self, s: &str) {
        self.marked.clear();
        self.marked.push_str(s);
    }

    /// `unmarkText` — composing is over, clear the preedit.
    pub fn unmark(&mut self) {
        self.marked.clear();
    }

    /// `insertText:` — the IME committed `s`. Ends any marked state, then either
    /// accumulates (inside a key event) or returns the text for immediate send.
    pub fn commit<'a>(&mut self, s: &'a str) -> Option<&'a str> {
        self.unmark();
        if self.in_key_event {
            self.committed.push(s.to_string());
            None
        } else {
            Some(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_insert_outside_key_event_sends_immediately() {
        let mut p = Preedit::new();
        assert_eq!(p.commit("a"), Some("a"));
        assert!(!p.is_composing());
    }

    #[test]
    fn marked_then_unmark() {
        let mut p = Preedit::new();
        p.set_marked("\u{3131}");
        assert!(p.is_composing());
        assert_eq!(p.marked_text(), "\u{3131}");
        p.unmark();
        assert!(!p.is_composing());
    }

    #[test]
    fn commit_during_key_event_is_accumulated_then_drained() {
        let mut p = Preedit::new();
        p.begin_key_event();
        assert_eq!(p.commit("\u{ac00}"), None);
        let drained = p.end_key_event();
        assert_eq!(drained, vec!["\u{ac00}".to_string()]);
    }

    #[test]
    fn dead_key_then_compose_e_acute() {
        let mut p = Preedit::new();
        p.begin_key_event();
        p.set_marked("\u{00b4}");
        let drained = p.end_key_event();
        assert!(drained.is_empty());
        assert!(p.is_composing());

        p.begin_key_event();
        assert_eq!(p.commit("\u{00e9}"), None);
        let drained = p.end_key_event();
        assert_eq!(drained, vec!["\u{00e9}".to_string()]);
        assert!(!p.is_composing());
    }
}
