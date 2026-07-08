//! Cursor style resolution. Port of `src/renderer/cursor.zig` (commit
//! `2da015cd6`).
//!
//! Upstream's `style()` takes a `*const terminal.RenderState` (a live
//! terminal handle). This crate has no such type — the closest thing is
//! `qwertty_term_vt::snapshot::SnapshotCursor`, an already-copied, borrow-free
//! cursor snapshot. [`CursorState`] adapts that (plus the two fields
//! upstream's `RenderState.cursor` carries that aren't yet wired in
//! `qwertty-term-vt` — see `docs/analysis/renderer-r0.md`) into `style()`'s input.

use qwertty_term_vt::snapshot::SnapshotCursor;

/// Available cursor styles for drawing that renderers must support. This is
/// a superset of terminal cursor styles since the renderer supports some
/// additional cursor states such as the hollow block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    // Typical cursor input styles.
    Block,
    BlockHollow,
    Bar,
    Underline,

    // Special cursor styles.
    Lock,
}

impl Style {
    /// Create a cursor style from the terminal style request.
    pub fn from_terminal(term: qwertty_term_vt::screen::cursor::CursorStyle) -> Style {
        use qwertty_term_vt::screen::cursor::CursorStyle as T;
        match term {
            T::Bar => Style::Bar,
            T::Block => Style::Block,
            T::BlockHollow => Style::BlockHollow,
            T::Underline => Style::Underline,
        }
    }
}

/// The renderer-local cursor state that `qwertty-term-vt`'s snapshot doesn't
/// carry on its own: whether the terminal is in preedit (IME composition),
/// whether the surface is focused, and whether the current blink phase
/// should show the cursor.
#[derive(Debug, Clone, Copy, Default)]
pub struct StyleOptions {
    pub preedit: bool,
    pub focused: bool,
    pub blink_visible: bool,
}

/// The cursor's position within the currently rendered viewport, in grid
/// (column, row) units. A local, minimal stand-in for upstream's
/// `RenderState.cursor.viewport` — `qwertty-term-vt` doesn't currently model
/// "is the cursor scrolled out of view" (see `docs/analysis/renderer-r0.md`),
/// so this only needs to carry enough to prove presence/absence for R0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorViewport {
    pub col: usize,
    pub row: usize,
}

/// The subset of cursor state `style()` needs, adapted from
/// [`SnapshotCursor`] plus fields not yet wired through `qwertty-term-vt` (see
/// the field docs below and `docs/analysis/renderer-r0.md`'s cursor.zig
/// section for what's missing and why).
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    /// Whether the cursor is within the currently rendered viewport. `None`
    /// when scrolled into history and thus not drawable at all.
    pub viewport: Option<CursorViewport>,
    /// Whether the terminal wants the cursor drawn at all (DECTCEM / mode
    /// 25).
    pub visible: bool,
    /// Whether the terminal is blinking the cursor (mode 12).
    pub blinking: bool,
    /// Whether the current input is a password (masked) field. No
    /// `qwertty-term-vt` producer wires this yet; callers not sourcing it should
    /// pass `false`.
    pub password_input: bool,
    /// The terminal-requested visual style (DECSCUSR).
    pub visual_style: qwertty_term_vt::screen::cursor::CursorStyle,
}

impl CursorState {
    /// Build cursor state from a snapshot cursor, given the render-side
    /// blinking mode. `viewport` is always `Some` (the active area, where a
    /// live cursor snapshot's cursor always lives); `password_input` is
    /// always `false` (no producer wired yet — see
    /// `docs/analysis/renderer-r0.md`).
    pub fn from_snapshot_cursor(cursor: &SnapshotCursor, blinking: bool) -> CursorState {
        CursorState {
            viewport: Some(CursorViewport {
                col: cursor.col,
                row: cursor.row,
            }),
            visible: cursor.visible,
            blinking,
            password_input: false,
            visual_style: cursor.style,
        }
    }
}

/// Returns the cursor style to use for the current render state, or `None`
/// if a cursor should not be rendered at all.
///
/// Note the order of conditionals below is important. It represents a
/// priority system of how we determine what state overrides cursor
/// visibility and style.
pub fn style(state: &CursorState, opts: StyleOptions) -> Option<Style> {
    // The cursor must be visible in the viewport to be rendered.
    state.viewport?;

    // If we are in preedit, then we always show the block cursor. We do
    // this even if the cursor is explicitly not visible because it shows
    // an important editing state to the user.
    if opts.preedit {
        return Some(Style::Block);
    }

    // If we're at a password input its always a lock.
    if state.password_input {
        return Some(Style::Lock);
    }

    // If the cursor is explicitly not visible by terminal mode, we don't
    // render.
    if !state.visible {
        return None;
    }

    // If we're not focused, our cursor is always visible so that we can
    // show the hollow box.
    if !opts.focused {
        return Some(Style::BlockHollow);
    }

    // If the cursor is blinking and our blink state is not visible, then we
    // don't show the cursor.
    if state.blinking && !opts.blink_visible {
        return None;
    }

    // Otherwise, we use whatever style the terminal wants.
    Some(Style::from_terminal(state.visual_style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwertty_term_vt::screen::cursor::CursorStyle;
    use qwertty_term_vt::stream::{Stream, TerminalHandler};
    use qwertty_term_vt::terminal::{Options, Terminal};

    fn feed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
        let term = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(bytes);
        stream.handler.terminal
    }

    fn opts(preedit: bool, focused: bool, blink_visible: bool) -> StyleOptions {
        StyleOptions {
            preedit,
            focused,
            blink_visible,
        }
    }

    #[test]
    fn default_uses_configured_style() {
        // Set the cursor style directly, matching upstream's test which
        // pokes `cursor_style` directly rather than going through DECSCUSR.
        let mut term = feed(10, 10, b"");
        term.screen_mut().cursor.cursor_style = CursorStyle::Bar;
        term.modes
            .set(qwertty_term_vt::modes::Mode::CursorBlinking, true);

        let snap = term.snapshot();
        let state = CursorState::from_snapshot_cursor(&snap.cursor, true);

        assert_eq!(style(&state, opts(false, true, true)), Some(Style::Bar));
        assert_eq!(
            style(&state, opts(false, false, true)),
            Some(Style::BlockHollow)
        );
        assert_eq!(
            style(&state, opts(false, false, false)),
            Some(Style::BlockHollow)
        );
        assert_eq!(style(&state, opts(false, true, false)), None);
    }

    #[test]
    fn blinking_disabled() {
        let mut term = feed(10, 10, b"");
        term.screen_mut().cursor.cursor_style = CursorStyle::Bar;
        term.modes
            .set(qwertty_term_vt::modes::Mode::CursorBlinking, false);

        let snap = term.snapshot();
        let state = CursorState::from_snapshot_cursor(&snap.cursor, false);

        assert_eq!(style(&state, opts(false, true, true)), Some(Style::Bar));
        assert_eq!(style(&state, opts(false, true, false)), Some(Style::Bar));
        assert_eq!(
            style(&state, opts(false, false, true)),
            Some(Style::BlockHollow)
        );
        assert_eq!(
            style(&state, opts(false, false, false)),
            Some(Style::BlockHollow)
        );
    }

    #[test]
    fn explicitly_not_visible() {
        let mut term = feed(10, 10, b"\x1b[?25l");
        term.screen_mut().cursor.cursor_style = CursorStyle::Bar;
        term.modes
            .set(qwertty_term_vt::modes::Mode::CursorBlinking, false);

        let snap = term.snapshot();
        let state = CursorState::from_snapshot_cursor(&snap.cursor, false);

        assert_eq!(style(&state, opts(false, true, true)), None);
        assert_eq!(style(&state, opts(false, true, false)), None);
        assert_eq!(style(&state, opts(false, false, true)), None);
        assert_eq!(style(&state, opts(false, false, false)), None);
    }

    #[test]
    fn always_block_with_preedit() {
        let term = feed(10, 10, b"");
        let snap = term.snapshot();
        let state = CursorState::from_snapshot_cursor(&snap.cursor, false);

        // In any bool state.
        assert_eq!(style(&state, opts(true, false, false)), Some(Style::Block));
        assert_eq!(style(&state, opts(true, true, false)), Some(Style::Block));
        assert_eq!(style(&state, opts(true, true, true)), Some(Style::Block));
        assert_eq!(style(&state, opts(true, false, true)), Some(Style::Block));

        // If we're scrolled though, then we don't show the cursor: model
        // that as `viewport: None` (see `CursorState` docs — `qwertty-term-vt`
        // doesn't yet report scrolled-out-of-view cursors on its own, so
        // this constructs the state directly rather than through a scrolled
        // snapshot).
        let scrolled = CursorState {
            viewport: None,
            ..state
        };

        assert_eq!(style(&scrolled, opts(true, false, false)), None);
        assert_eq!(style(&scrolled, opts(true, true, false)), None);
        assert_eq!(style(&scrolled, opts(true, true, true)), None);
        assert_eq!(style(&scrolled, opts(true, false, true)), None);
    }
}
