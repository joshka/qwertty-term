//! The platform-free half of mouse selection + clipboard paste, factored out of
//! the GTK event handlers so it unit-tests without a running GTK/Xvfb loop.
//!
//! Two pure functions live here:
//!
//! - [`pixel_to_cell`]: GLArea pixel → grid `(col, row)`. The GTK gesture
//!   handlers translate a pointer position into a cell here, then hand the cell
//!   to the reused [`qwertty_term::gesture::SelectionGesture`] +
//!   [`qwertty_term::engine::Engine`] selection API. Mirrors upstream's
//!   pixel→grid mapping in `class/surface.zig` (`cursorPosCallback` /
//!   `mouseButtonCallback` compute `x / cell_width`, `y / cell_height`); this
//!   host has no window padding yet, so it is a bare floor+clamp.
//!
//! - [`encode_paste`]: clipboard text → the exact bytes to write to the pty,
//!   honoring bracketed-paste mode (`ESC [ 200 ~` … `ESC [ 201 ~` when the
//!   running program enabled it — the terminal's `bracketed_paste()` mode).
//!   Port of upstream's paste framing in `Surface.zig` (`paste` /
//!   `completeClipboardRequest` wrap the data in the bracketed sequence when
//!   `modes.get(.bracketed_paste)` is set).
//!
//! Paste *safety* (multiline / smuggled-end-sequence classification) reuses
//! [`qwertty_term::paste`] directly; the selection gestures reuse
//! [`qwertty_term::gesture`]; the menu model reuses
//! [`qwertty_term::context_menu`]. This module adds only the two mappings those
//! shared crates don't provide.

/// Bracketed-paste start/end framing (DEC mode 2004). Data pasted while the
/// running program has bracketed paste on is wrapped in these so the program
/// can tell pasted bytes from typed ones.
const BRACKET_START: &[u8] = b"\x1b[200~";
const BRACKET_END: &[u8] = b"\x1b[201~";

/// Map a GLArea pixel coordinate to a grid cell `(col, row)`, clamped into the
/// `cols` × `rows` grid. `cell_w`/`cell_h` are the cell metrics in the same
/// (device) pixel space as `x`/`y`. Negative inputs clamp to 0 (a pointer that
/// left the top/left edge during a drag still maps to the first cell).
///
/// Reduced port of the upstream pixel→grid math (`class/surface.zig`
/// `cursorPosCallback`): no left/top padding is wired into this host, so the
/// cell is simply `floor(x / cell_w)` clamped to `[0, cols-1]` (likewise for the
/// row). A zero cell size (never happens for a real font) is treated as one
/// pixel so this can't divide by zero.
pub fn pixel_to_cell(
    x: f64,
    y: f64,
    cell_w: usize,
    cell_h: usize,
    cols: usize,
    rows: usize,
) -> (usize, usize) {
    let cw = cell_w.max(1) as f64;
    let ch = cell_h.max(1) as f64;
    let col = if x <= 0.0 { 0 } else { (x / cw) as usize };
    let row = if y <= 0.0 { 0 } else { (y / ch) as usize };
    (
        col.min(cols.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
    )
}

/// Encode clipboard `text` into the bytes to write to the pty. When `bracketed`
/// (the terminal has DEC mode 2004 on), the payload is framed with the
/// bracketed-paste start/end sequences; otherwise the raw UTF-8 bytes are sent
/// as-is. Port of upstream's paste framing (`Surface.zig` wraps the data in
/// `ESC [ 200 ~` … `ESC [ 201 ~` iff `bracketed_paste` is set).
///
/// Safety classification (whether a multiline / end-sequence-smuggling paste
/// should be confirmed first) is a separate concern handled by
/// [`qwertty_term::paste::is_unsafe`]; this function performs the transport
/// encoding only.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.as_bytes();
    if !bracketed {
        return body.to_vec();
    }
    let mut out = Vec::with_capacity(body.len() + BRACKET_START.len() + BRACKET_END.len());
    out.extend_from_slice(BRACKET_START);
    out.extend_from_slice(body);
    out.extend_from_slice(BRACKET_END);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pixel_to_cell ----------------------------------------------------

    #[test]
    fn pixel_to_cell_maps_coordinates() {
        // 10px-wide, 20px-tall cells over an 80×24 grid.
        assert_eq!(pixel_to_cell(0.0, 0.0, 10, 20, 80, 24), (0, 0));
        // x=25 → col 2 (floor 2.5); y=45 → row 2 (floor 2.25).
        assert_eq!(pixel_to_cell(25.0, 45.0, 10, 20, 80, 24), (2, 2));
        // Exact cell boundary: x=30 → col 3, y=40 → row 2.
        assert_eq!(pixel_to_cell(30.0, 40.0, 10, 20, 80, 24), (3, 2));
    }

    #[test]
    fn pixel_to_cell_clamps_to_grid() {
        // Past the right/bottom edge → last col/row.
        assert_eq!(pixel_to_cell(10_000.0, 10_000.0, 10, 20, 80, 24), (79, 23));
        // Negative (pointer left the top-left during a drag) → first cell.
        assert_eq!(pixel_to_cell(-5.0, -5.0, 10, 20, 80, 24), (0, 0));
    }

    #[test]
    fn pixel_to_cell_zero_cell_size_is_safe() {
        // Never happens for a real font, but must not divide by zero.
        assert_eq!(pixel_to_cell(5.0, 5.0, 0, 0, 80, 24), (5, 5));
    }

    // ---- selection → text (reuses the app-crate Engine selection API) -----

    /// Drive the reused terminal [`Engine`](qwertty_term::engine::Engine)
    /// selection over known content and assert the extracted text — the same
    /// `select_screen_points` → `selection_string` path the GTK copy handler
    /// runs, with no GTK event injection.
    #[test]
    fn selection_extracts_expected_text() {
        let mut engine = qwertty_term::engine::Engine::new(20, 3);
        engine.write(b"hello world");
        // Select cols 0..=4 on row 0 → "hello".
        assert!(engine.select_screen_points((0, 0), (4, 0), false));
        assert_eq!(engine.selection_string().as_deref(), Some("hello"));
        // Extend the selection across the whole word run → "hello world".
        assert!(engine.select_screen_points((0, 0), (10, 0), false));
        assert_eq!(engine.selection_string().as_deref(), Some("hello world"));
        // Clearing drops the selection text.
        engine.clear_selection();
        assert_eq!(engine.selection_string(), None);
    }

    // ---- paste encoding ---------------------------------------------------

    #[test]
    fn encode_paste_plain_is_raw_bytes() {
        assert_eq!(encode_paste("ls -la", false), b"ls -la".to_vec());
        assert_eq!(encode_paste("", false), Vec::<u8>::new());
    }

    #[test]
    fn encode_paste_bracketed_wraps_payload() {
        let out = encode_paste("ls -la", true);
        assert_eq!(out, b"\x1b[200~ls -la\x1b[201~".to_vec());
        // Empty text still emits the frame (matches upstream: the program sees
        // an empty bracketed paste rather than nothing).
        assert_eq!(encode_paste("", true), b"\x1b[200~\x1b[201~".to_vec());
    }

    /// Prove the encoded paste bytes actually traverse the pty write side — the
    /// transport end of the copy/paste path, exercised against a real pty the
    /// same way the keyboard round-trip test does. The exact *bracketed* framing
    /// is asserted deterministically above by [`encode_paste_bracketed_wraps_payload`];
    /// here we send the plain (unbracketed) encoding of a printable payload and
    /// read it back through the pty's line-discipline echo, so the assertion
    /// isn't confused by control-byte (`^[`) echo rendering.
    #[test]
    fn paste_bytes_reach_pty() {
        use qwertty_term_termio::pty::{Pty, Winsize};
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let pty = Pty::open(Winsize::default()).expect("openpty");
        let (master, slave) = pty.into_parts();
        let master_read = master.try_clone().expect("clone master");
        let mut writer = std::fs::File::from(master);

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let reader_buf = Arc::clone(&buf);
        let reader = std::thread::spawn(move || {
            let mut f = std::fs::File::from(master_read);
            let mut chunk = [0u8; 1024];
            loop {
                match f.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader_buf.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
        });

        // Non-bracketed paste of a printable payload → raw bytes on the pty.
        let payload = encode_paste("echo hi", false);
        assert_eq!(payload, b"echo hi".to_vec());
        writer.write_all(&payload).expect("write to pty");
        writer.flush().ok();

        let want = b"echo hi";
        let deadline = Instant::now() + Duration::from_secs(5);
        let found = loop {
            if buf.lock().unwrap().windows(want.len()).any(|w| w == want) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let got = buf.lock().unwrap().clone();
        drop(slave);
        let _ = reader.join();

        assert!(found, "pty did not carry the paste bytes (got {got:?})");
    }
}
