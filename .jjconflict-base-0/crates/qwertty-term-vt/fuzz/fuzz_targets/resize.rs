//! Fuzz target: byte-chunks interleaved with grid resizes.
//!
//! Motivation: the pure-bytes `parser` target (and the 7.9M-run campaign it
//! drove) never resizes the grid, so it was blind to a class of field crashes
//! where a resize desyncs the cursor/pin from the page list. The canonical
//! example was a release-only panic in `Screen::cursor_absolute` after a
//! larger resize followed by an alt-screen (DEC 1049) enter + CUP to a corner.
//!
//! This target interprets the fuzz input as a *script* of operations against a
//! real `Terminal` driven through the `Stream`/`TerminalHandler` the app uses:
//!
//!   * feed a chunk of raw pty bytes, or
//!   * resize the grid to a new (cols, rows), clamped to a sane range.
//!
//! Invariant: no operation sequence may panic. Resizes are derived from the
//! input bytes and clamped to `1..=MAX_DIM` so we never construct a degenerate
//! grid, matching what a windowing system would actually deliver.
//!
//! Run (nightly + cargo-fuzz required):
//!   cargo +nightly fuzz run resize -- -max_total_time=600

#![no_main]

use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};
use libfuzzer_sys::fuzz_target;

/// Clamp for resize dimensions. Real windowing systems never deliver a
/// zero-sized or absurdly huge grid; keep resizes in a band that exercises
/// both grow and shrink (relative to the 80x24 start) without spending the
/// whole budget allocating enormous pages.
const MAX_DIM: u16 = 500;

fn clamp_dim(raw: u16) -> u16 {
    (raw % MAX_DIM).max(1)
}

/// Interpret `data` as a script and drive it against a fresh terminal.
///
/// Wire format (self-describing, total over any byte string — a truncated
/// trailing op is simply ignored, so every input is valid):
///
///   op := tag:u8 body
///   tag & 0b1 == 0  => feed op: body = len:u8, then `len` raw bytes
///   tag & 0b1 == 1  => resize op: body = cols_lo cols_hi rows_lo rows_hi
///                      (little-endian u16 each), clamped to 1..=MAX_DIM
///
/// The high bits of `tag` are unused, so the fuzzer freely mutates them; only
/// the low bit picks the op. Feed lengths are capped at 255 by the u8 length
/// prefix, which keeps individual chunks small and resizes frequent.
fn run_script(data: &[u8]) {
    let term = Terminal::new(Options {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));

    let mut i = 0usize;
    while i < data.len() {
        let tag = data[i];
        i += 1;
        if tag & 1 == 0 {
            // Feed op.
            let len = match data.get(i) {
                Some(&l) => l as usize,
                None => break,
            };
            i += 1;
            let end = (i + len).min(data.len());
            stream.feed(&data[i..end]);
            i = end;
        } else {
            // Resize op: need 4 bytes.
            if i + 4 > data.len() {
                break;
            }
            let cols = clamp_dim(u16::from_le_bytes([data[i], data[i + 1]]));
            let rows = clamp_dim(u16::from_le_bytes([data[i + 2], data[i + 3]]));
            i += 4;
            stream.handler.terminal.resize(cols, rows);
        }
    }

    // Touch cursor-absolute-adjacent state after the script so a latent
    // desync surfaces here even if the last op didn't dereference it.
    std::hint::black_box(stream.handler.terminal.screen().cursor.x);
    std::hint::black_box(stream.handler.terminal.screen().cursor.y);
}

fuzz_target!(|data: &[u8]| {
    run_script(data);
});
