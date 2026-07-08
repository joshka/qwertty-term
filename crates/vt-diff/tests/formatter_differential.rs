//! Formatter differential test: the ported Rust formatter
//! ([`qwertty_term_vt::formatter`]) vs the Zig `ghostty_formatter_terminal_*`
//! reference, comparing **plain-text formatter output** (a richer comparison
//! than the screen-text differ — it exercises trim, blank-row/blank-cell
//! accounting, and wide-char handling in the serializer itself).
//!
//! Only runs with the `reference` feature (links the Zig static lib):
//! `cargo test -p vt-diff --features reference`.

#![cfg(feature = "reference")]

use std::fs;
use std::path::Path;

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal, decode_escaped_stream};

/// Feed `input` to both engines and assert the formatter's plain dumps agree
/// after the shared normalization (the reference dump can carry trailing blank
/// rows / trailing whitespace that both sides drop identically).
fn assert_formatter_agree(label: &str, cols: u16, rows: u16, input: &[u8]) {
    let mut reference = ReferenceTerminal::new(cols, rows);
    let mut rust = RustTerminal::new(cols, rows);
    reference.feed(input);
    rust.feed(input);

    let rref = vt_diff::normalize_screen_text(&reference.raw_text());
    let rrust = vt_diff::normalize_screen_text(&rust.formatter_raw_text());
    assert_eq!(rref, rrust, "FORMATTER diverged for `{label}`");
}

#[test]
fn fixtures_formatter_agree() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../spike/tests/fixtures/replay")
        .canonicalize()
        .expect("replay fixture directory exists");

    let mut ran = 0;
    for entry in fs::read_dir(&fixtures).expect("read replay fixture directory") {
        let path = entry.expect("read replay fixture entry").path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let (cols, rows) = read_size(&path.join("size.txt"));
        let input = fs::read_to_string(path.join("input.esc")).expect("read fixture input");
        assert_formatter_agree(&name, cols, rows, &decode_escaped_stream(&input));
        ran += 1;
    }
    assert!(ran >= 3, "expected at least 3 fixtures, ran {ran}");
}

#[test]
fn hand_streams_formatter_agree() {
    assert_formatter_agree("wrap", 8, 4, b"abcdefghij");
    assert_formatter_agree(
        "scroll_region",
        20,
        6,
        b"\x1B[2;4r\x1B[2;1Hone\r\ntwo\r\nthree\r\nfour\r\nfive",
    );
    assert_formatter_agree(
        "sgr",
        30,
        3,
        b"\x1B[1;38;5;196mRED\x1B[0m normal \x1B[4munder\x1B[0m",
    );
    assert_formatter_agree("wide", 12, 3, "\u{597d}\u{4e16}\u{754c}!".as_bytes());
    assert_formatter_agree(
        "cursor_moves",
        20,
        5,
        b"line1\r\nline2\r\nline3\x1B[1;1H\x1B[2C\x1B[Kx\x1B[2;3Hy",
    );
    assert_formatter_agree("tabs_erase", 40, 3, b"a\tb\tc\r\n\x1B[1;1H\x1B[0Kreplaced");
    assert_formatter_agree("trailing_ws", 20, 3, b"hello   \r\nworld  \r\n\r\n");
    assert_formatter_agree("multi_blank", 20, 6, b"a\r\n\r\n\r\nb");
}

// ---- fixture-loading helpers (kept in sync with differential.rs) --------

fn read_size(path: &Path) -> (u16, u16) {
    let text = fs::read_to_string(path).expect("read fixture size");
    let mut parts = text.split_whitespace();
    let cols = parts.next().unwrap().parse().unwrap();
    let rows = parts.next().unwrap().parse().unwrap();
    (cols, rows)
}
