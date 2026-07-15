//! Regression test for the field crash: entering the alternate screen after the
//! terminal has been resized larger must not panic.
//!
//! Root cause: the alternate screen was created lazily with the terminal's
//! *construction-time* dimensions instead of its current (resized) dimensions.
//! Copying the primary cursor (which could sit on a row that only exists at the
//! larger size) into the stale-sized alternate screen walked the cursor pin off
//! the page list. In release builds the bounds checks in `Screen::cursor_absolute`
//! are `debug_assert!` (compiled out), so the pin-walk `.up()/.down().unwrap()`
//! panicked on `None` — crashing the io-reader thread and poisoning the engine
//! mutex.
//!
//! Arbitrary pty bytes must never panic qwertty-term-vt, so this is a hard invariant.
//!
//! These tests are meaningful in BOTH debug and release, but the crash they guard
//! only manifested in release (debug_assert would have aborted earlier with a
//! clearer message). They are part of the `cargo test -p qwertty-term-vt --release`
//! gate.

use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

fn stream(cols: u16, rows: u16) -> Stream<TerminalHandler> {
    let t = Terminal::new(Options {
        cols,
        rows,
        max_scrollback: 10_000,
        ..Default::default()
    });
    Stream::new(TerminalHandler::new(t))
}

/// The exact minimal field-crash sequence: start at 80x24, resize larger (the
/// window-appearance resize the app performs at launch), enter the alternate
/// screen via DEC 1049, then a CUP to the far corner. Before the fix this
/// panicked in release inside `cursor_absolute`.
#[test]
fn resize_larger_then_enter_alt_screen_then_cup() {
    let mut s = stream(80, 24);
    // Window-appearance resize: grow to a taller/wider grid.
    s.handler.terminal.resize(120, 40);
    // Shell/app enters the alternate screen (e.g. a full-screen program, or the
    // 1049 save-cursor-clear-enter used during startup).
    s.feed(b"\x1b[?1049h");
    // Cursor move to the bottom-right of the now-larger alternate screen.
    s.feed(b"\x1b[999;999H");
    // Some output on the alternate screen.
    s.feed(b"hello from the alt screen\r\n");

    // The alternate screen must have adopted the current dimensions.
    assert_eq!(s.handler.terminal.cols, 120);
    assert_eq!(s.handler.terminal.rows, 40);
}

/// Broader coverage: resizing while already on the alternate screen, and the
/// 1047 legacy alt path, must also stay in bounds after a large resize.
#[test]
fn alt_screen_survives_resize_grow_and_cursor_moves() {
    let mut s = stream(80, 24);
    s.feed(b"\x1b[?1049h");
    // Grow while on the alt screen.
    s.handler.terminal.resize(200, 60);
    s.feed(b"\x1b[999;999H");
    s.feed(b"\x1b[H");
    s.feed(b"\x1b[60;200H");
    // Leave and re-enter to exercise re-init at the new size.
    s.feed(b"\x1b[?1049l");
    s.handler.terminal.resize(80, 24);
    s.feed(b"\x1b[?1047h");
    s.feed(b"\x1b[24;80H");
}

/// A resize that grows rows while the cursor is well down the primary screen,
/// then enters the alt screen: the copied cursor y must be representable on the
/// alternate screen.
#[test]
fn primary_cursor_deep_then_alt_screen() {
    let mut s = stream(80, 24);
    // Fill the screen so the cursor is at the bottom.
    for i in 0..24 {
        s.feed(format!("line {i}\r\n").as_bytes());
    }
    s.handler.terminal.resize(100, 50);
    // Move cursor deep on the primary.
    s.feed(b"\x1b[45;90H");
    // Enter the alt screen; cursor copy must land in bounds.
    s.feed(b"\x1b[?1049h");
    s.feed(b"content\r\n");
    // Back to primary.
    s.feed(b"\x1b[?1049l");
}
