//! Differential test: pure-Rust `ghostty-vt` vs the Zig `libghostty-vt`
//! reference, comparing screen text + cursor position.
//!
//! Only runs with the `reference` feature (links the Zig-built static lib):
//! `cargo test -p vt-diff --features reference`.

#![cfg(feature = "reference")]

use std::fs;
use std::path::Path;

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal, ScreenDump};

/// Feed `input` to both oracles at `cols`x`rows` and assert identical
/// observable state. Returns the shared dump for further inspection.
fn assert_agree(label: &str, cols: u16, rows: u16, input: &[u8]) -> ScreenDump {
    let mut reference = ReferenceTerminal::new(cols, rows);
    let mut rust = RustTerminal::new(cols, rows);
    reference.feed(input);
    rust.feed(input);

    let rd = reference.dump();
    let ud = rust.dump();
    assert_eq!(rd.text, ud.text, "TEXT diverged for `{label}`");
    assert_eq!(rd.cursor, ud.cursor, "CURSOR diverged for `{label}`");
    rd
}

// ---- fixtures -----------------------------------------------------------

#[test]
fn fixtures_agree() {
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
        assert_agree(&name, cols, rows, &decode_escaped_stream(&input));
        ran += 1;
    }
    assert!(ran >= 3, "expected at least 3 fixtures, ran {ran}");
}

// ---- hand-written streams ----------------------------------------------

#[test]
fn hand_wrap() {
    // Soft-wrap: 10 chars into an 8-column terminal.
    assert_agree("wrap", 8, 4, b"abcdefghij");
}

#[test]
fn hand_scroll_region() {
    // DECSTBM scroll region rows 2..4, then several linefeeds to force
    // scrolling within the region only.
    assert_agree(
        "scroll_region",
        20,
        6,
        b"\x1B[2;4r\x1B[2;1Hone\r\ntwo\r\nthree\r\nfour\r\nfive",
    );
}

#[test]
fn hand_alt_screen() {
    assert_agree(
        "alt_screen",
        20,
        5,
        b"primary\r\n\x1B[?1049h\x1B[2J\x1B[Halt content\x1B[?1049lback",
    );
}

#[test]
fn hand_sgr() {
    // SGR set + reset around text; text content must match (styles aren't in
    // the text comparison but the cursor/print flow must agree).
    assert_agree(
        "sgr",
        30,
        3,
        b"\x1B[1;38;5;196mRED\x1B[0m normal \x1B[4munder\x1B[0m",
    );
}

#[test]
fn hand_wide_chars() {
    // Wide (CJK) chars advance two columns each.
    assert_agree("wide", 12, 3, "\u{597d}\u{4e16}\u{754c}!".as_bytes());
}

#[test]
fn hand_cursor_moves() {
    // A mix of absolute/relative cursor motion + erases.
    assert_agree(
        "cursor_moves",
        20,
        5,
        b"line1\r\nline2\r\nline3\x1B[1;1H\x1B[2C\x1B[Kx\x1B[2;3Hy",
    );
}

#[test]
fn hand_tabs_and_erase() {
    assert_agree("tabs_erase", 40, 3, b"a\tb\tc\r\n\x1B[1;1H\x1B[0Kreplaced");
}

#[test]
fn hand_insert_delete() {
    assert_agree(
        "insert_delete",
        20,
        3,
        b"abcdef\x1B[1;3H\x1B[2@XY\x1B[1;1H\x1B[2P",
    );
}

// ---- fixture-loading helpers (kept in sync with smoke.rs) --------------

fn read_size(path: &Path) -> (u16, u16) {
    let text = fs::read_to_string(path).expect("read fixture size");
    let mut parts = text.split_whitespace();
    let cols = parts.next().unwrap().parse().unwrap();
    let rows = parts.next().unwrap().parse().unwrap();
    (cols, rows)
}

fn decode_escaped_stream(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hi = chars.next().unwrap().to_digit(16).unwrap() as u8;
                let lo = chars.next().unwrap().to_digit(16).unwrap() as u8;
                out.push((hi << 4) | lo);
            }
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}
