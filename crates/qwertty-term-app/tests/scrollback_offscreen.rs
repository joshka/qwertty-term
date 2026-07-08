//! Offscreen scrollback evidence test (no GUI, no Metal, no PTY).
//!
//! Drives the real `qwertty_term_app::engine::Engine` (a thin wrapper over the real
//! `qwertty-term-vt` terminal), fills the scrollback of a small grid with uniquely
//! numbered lines, and asserts that the windowed snapshot the render path
//! consumes (`snapshot_window(offset)`) shows *history* rows when the viewport
//! offset is raised and snaps back to the live tail at offset 0.
//!
//! This exercises exactly the seam the wheel-scroll ladder drives: a per-pane
//! `scrollback_offset` fed into `snapshot_window`. The AppKit `scrollWheel:` →
//! `Controller::wheel_to_surface` plumbing on top is covered by the pure
//! `qwertty_term_app::scroll` unit tests (the decision ladder + accumulator math)
//! and the windowed splits smoke; this test proves the offset actually pulls
//! the correct history rows into the rendered frame.

use qwertty_term_app::engine::Engine;
use qwertty_term_app::input::mouse::{self, MouseContext};
use qwertty_term_app::scroll::{ScrollMultiplier, WheelOutcome, WheelState, decide};
use qwertty_term_input::key_mods::Mods;
use qwertty_term_input::mouse::{Action, Button};
use qwertty_term_input::mouse_encode::MouseEvent;

/// Concatenate a snapshot row's cell characters into a trimmed string.
fn row_text(row: &qwertty_term_vt::snapshot::SnapshotRow) -> String {
    row.cells
        .iter()
        .map(|c| c.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The first (topmost) visible row's text for a given scrollback offset.
fn top_row_text(engine: &Engine, offset: usize) -> String {
    let window = engine.snapshot_window(offset);
    row_text(&window.window[0])
}

/// Every visible row's text for a given offset.
fn window_rows(engine: &Engine, offset: usize) -> Vec<String> {
    engine
        .snapshot_window(offset)
        .window
        .iter()
        .map(row_text)
        .collect()
}

#[test]
fn scrollback_offset_reveals_history_and_snaps_back() {
    let rows = 10;
    let cols = 40;
    let mut engine = Engine::new(cols, rows);

    // Feed 200 uniquely-numbered lines. With a 10-row grid this pushes ~190
    // lines into scrollback. `\r\n` so each `LINE-N` starts at column 0.
    let total = 200usize;
    let mut input = String::new();
    for i in 0..total {
        input.push_str(&format!("LINE-{i:03}\r\n"));
    }
    engine.write(input.as_bytes());

    // At offset 0 the viewport shows the live tail: the last line written was
    // "LINE-199\r\n", leaving the cursor on a fresh blank final row, so the
    // last *content* row is LINE-199.
    let tail = window_rows(&engine, 0);
    assert!(
        tail.iter().any(|r| r == "LINE-199"),
        "offset 0 should show the newest line; got {tail:?}"
    );
    assert!(
        !tail.iter().any(|r| r == "LINE-050"),
        "offset 0 must NOT show old history; got {tail:?}"
    );

    // There should be plenty of scrollback to scroll into.
    let scrollback = engine.scrollback_len();
    assert!(
        scrollback >= 180,
        "expected ~190 rows of scrollback, got {scrollback}"
    );

    // Scroll up far enough to bring a known marker from ~line 50 into view.
    // The bottom-most content row is LINE-199 at offset 0. LINE-050 is
    // (199 - 50) = 149 rows above the last content row. Offset counts rows up
    // from the very bottom (including the trailing blank cursor row), so an
    // offset of 150 lands LINE-050 near the top of the window.
    let offset = 150;
    let history = window_rows(&engine, offset);
    assert!(
        history.iter().any(|r| r == "LINE-050"),
        "offset {offset} should reveal LINE-050 from history; got {history:?}"
    );
    // And the live tail must no longer be visible.
    assert!(
        !history.iter().any(|r| r == "LINE-199"),
        "scrolled-back window must not show the live tail; got {history:?}"
    );

    // Offset clamps at the top of history: an absurd offset shows the oldest
    // retained rows, never panics, and always yields exactly `rows` entries.
    let clamped = engine.snapshot_window(usize::MAX);
    assert_eq!(clamped.window.len(), rows);
    let top = row_text(&clamped.window[0]);
    assert!(
        top.starts_with("LINE-"),
        "top of clamped history should be a real line, got {top:?}"
    );

    // Snapping back to offset 0 again shows the live tail (idempotent readback).
    assert_eq!(top_row_text(&engine, 0), window_rows(&engine, 0)[0]);
    assert!(window_rows(&engine, 0).iter().any(|r| r == "LINE-199"));
}

#[test]
fn wheel_delta_drives_offset_into_history_then_snaps_to_zero() {
    // End-to-end at the row-delta level (the part the AppKit view would call):
    // a discrete wheel-up tick yields a positive row delta; applying it as a
    // rows-up offset moves into history; a wheel-down / snap returns to 0. This
    // ties the `scroll` module's arithmetic to an offset the render path uses,
    // without needing AppKit.
    let rows = 10;
    let mut engine = Engine::new(40, rows);
    let mut feed = String::new();
    for i in 0..200 {
        feed.push_str(&format!("LINE-{i:03}\r\n"));
    }
    engine.write(feed.as_bytes());

    let cell_h = 16.0;
    let mult = ScrollMultiplier::default();
    let mut wheel = WheelState::default();
    let mut offset: usize = 0;
    let max = engine.scrollback_len();

    // Primary screen, no reporting, mode 1007 default true but not alt screen
    // → the ladder must choose the viewport path.
    let reporting = false;
    let alt_screen = engine.alt_screen_active(); // false on primary
    let alt_scroll = engine.mouse_alternate_scroll(); // true by default
    assert!(!alt_screen);
    assert!(alt_scroll);

    // Ten wheel-up ticks (positive yoff). Each discrete tick = 1*16*3 = 48px =
    // 3 rows, so 10 ticks = 30 rows up into history.
    for _ in 0..10 {
        let delta = wheel.row_delta(1.0, false, cell_h, mult);
        match decide(delta, reporting, alt_screen, alt_scroll) {
            WheelOutcome::Viewport { rows_up } => {
                offset = ((offset as isize + rows_up).clamp(0, max as isize)) as usize;
            }
            other => panic!("expected viewport scroll on primary screen, got {other:?}"),
        }
    }
    assert_eq!(offset, 30, "ten up-ticks * 3 rows should be offset 30");
    let history = engine
        .snapshot_window(offset)
        .window
        .iter()
        .map(row_text)
        .collect::<Vec<_>>();
    // Offset 30 rows up from the bottom shows lines around 199-30 = ~169.
    assert!(
        history.iter().any(|r| r.starts_with("LINE-1")),
        "scrolled-back window should show older lines; got {history:?}"
    );
    assert!(
        !history.iter().any(|r| r == "LINE-199"),
        "at offset 30 the very last line should be off the bottom; got {history:?}"
    );

    // A single key press snaps back to the bottom (upstream keystroke default).
    offset = 0;
    let tail = engine
        .snapshot_window(offset)
        .window
        .iter()
        .map(row_text)
        .collect::<Vec<_>>();
    assert!(tail.iter().any(|r| r == "LINE-199"));
}

/// Build the wheel-report context the surface uses (SGR at cell 0,0).
fn wheel_ctx(mode: MouseEvent) -> MouseContext {
    MouseContext {
        event_mode: mode,
        format: qwertty_term_input::mouse_encode::MouseFormat::Sgr,
        screen_width: 40.0 * 8.0,
        screen_height: 10.0 * 16.0,
        cell_width: 8.0,
        cell_height: 16.0,
        any_button_pressed: false,
    }
}

#[test]
fn mouse_reporting_wheel_emits_button_4_5_bytes_and_no_viewport_move() {
    // Enable SGR mouse reporting (normal tracking + SGR format) in a real
    // engine, exactly as a TUI would: ESC[?1000h (normal) + ESC[?1006h (SGR).
    let mut engine = Engine::new(40, 10);
    engine.write(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(engine.mouse_event(), MouseEvent::Normal);

    // The ladder must choose the reporting path (reporting active beats the
    // alt-scroll and viewport branches, even though mode 1007 defaults on).
    let reporting = engine.mouse_event() != MouseEvent::None;
    let out = decide(
        3,
        reporting,
        engine.alt_screen_active(),
        engine.mouse_alternate_scroll(),
    );
    assert_eq!(out, WheelOutcome::Report { count: 3, up: true });

    // Byte-compare: a wheel-up report is button 4 (code 64) press at cell 1;1.
    let mut last = None;
    let up_bytes = mouse::encode(
        Action::Press,
        Some(Button::Four),
        Mods::default(),
        0.0,
        0.0,
        &wheel_ctx(MouseEvent::Normal),
        &mut last,
    );
    assert_eq!(up_bytes, b"\x1b[<64;1;1M");

    // Wheel-down is button 5 (code 65).
    let mut last = None;
    let down_bytes = mouse::encode(
        Action::Press,
        Some(Button::Five),
        Mods::default(),
        0.0,
        0.0,
        &wheel_ctx(MouseEvent::Normal),
        &mut last,
    );
    assert_eq!(down_bytes, b"\x1b[<65;1;1M");
}

#[test]
fn alt_screen_alternate_scroll_emits_arrow_keys_per_deccm() {
    // Enter the alternate screen (ESC[?1049h). Mode 1007 defaults on and no
    // mouse reporting is set, so the ladder takes the alternate-scroll path.
    let mut engine = Engine::new(40, 10);
    engine.write(b"\x1b[?1049h");
    assert!(engine.alt_screen_active());
    assert!(engine.mouse_alternate_scroll());
    assert_eq!(engine.mouse_event(), MouseEvent::None);

    let reporting = engine.mouse_event() != MouseEvent::None;
    let up = decide(
        2,
        reporting,
        engine.alt_screen_active(),
        engine.mouse_alternate_scroll(),
    );
    assert_eq!(up, WheelOutcome::AltScrollKeys { count: 2, up: true });
    let down = decide(
        -2,
        reporting,
        engine.alt_screen_active(),
        engine.mouse_alternate_scroll(),
    );
    assert_eq!(
        down,
        WheelOutcome::AltScrollKeys {
            count: 2,
            up: false
        }
    );

    // DECCKM off (default): normal-mode cursor keys ESC[A / ESC[B.
    assert!(!engine.key_encode_options().cursor_key_application);

    // DECCKM on (ESC[?1h): application-mode cursor keys ESC O A / ESC O B.
    engine.write(b"\x1b[?1h");
    assert!(engine.key_encode_options().cursor_key_application);
}
