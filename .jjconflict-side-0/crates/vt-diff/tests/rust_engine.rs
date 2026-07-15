//! The Rust `qwertty-term-vt` oracle, exercised standalone (no reference library).
//!
//! Runs always (`cargo test -p vt-diff`); verifies the in-tree
//! [`RustTerminal`] oracle reproduces the replay fixtures. The reference
//! comparison lives in `differential.rs` behind the `reference` feature.

use std::fs;
use std::path::Path;

use vt_diff::{CursorPos, Oracle, RustTerminal, normalize_screen_text};

#[test]
fn hello_world_text_and_cursor() {
    let mut term = RustTerminal::new(20, 5);
    term.feed(b"hello\r\nworld");
    let dump = term.dump();
    assert_eq!(dump.text, "hello\nworld");
    assert_eq!(dump.cursor, CursorPos { row: 1, col: 5 });
}

#[test]
fn empty_terminal_dumps_empty_text() {
    let term = RustTerminal::new(10, 3);
    assert_eq!(term.text(), "");
    assert_eq!(term.cursor(), CursorPos { row: 0, col: 0 });
}

#[test]
fn replay_fixtures_match_expected_screen() {
    let fixtures = fixture_dir();
    let mut ran = 0;
    for entry in fs::read_dir(&fixtures).expect("read replay fixture directory") {
        let path = entry.expect("read replay fixture entry").path();
        if path.is_dir() {
            run_fixture(&path);
            ran += 1;
        }
    }
    assert!(ran >= 3, "expected at least 3 fixtures, ran {ran}");
}

fn run_fixture(path: &Path) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>");
    let (cols, rows) = read_size(&path.join("size.txt"));
    let input = fs::read_to_string(path.join("input.esc")).expect("read fixture input");
    let expected = fs::read_to_string(path.join("expected.txt")).expect("read fixture expected");

    let mut term = RustTerminal::new(cols, rows);
    term.feed(&decode_escaped_stream(&input));

    assert_eq!(
        term.text(),
        normalize_screen_text(&expected),
        "fixture {name}"
    );
}

// ---- shared fixture helpers (kept in sync with smoke.rs) ----------------

pub fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../spike/tests/fixtures/replay")
        .canonicalize()
        .expect("replay fixture directory exists")
}

pub fn read_size(path: &Path) -> (u16, u16) {
    let text = fs::read_to_string(path).expect("read fixture size");
    let mut parts = text.split_whitespace();
    let cols = parts.next().unwrap().parse().unwrap();
    let rows = parts.next().unwrap().parse().unwrap();
    (cols, rows)
}

pub fn decode_escaped_stream(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_utf8(&mut out, ch);
            continue;
        }
        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hi = chars.next().expect("hex escape high nibble");
                let lo = chars.next().expect("hex escape low nibble");
                out.push(hex_byte(hi, lo));
            }
            Some(other) => {
                out.push(b'\\');
                push_utf8(&mut out, other);
            }
            None => out.push(b'\\'),
        }
    }
    out
}

fn push_utf8(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

fn hex_byte(hi: char, lo: char) -> u8 {
    let hi = hi.to_digit(16).expect("valid high hex nibble") as u8;
    let lo = lo.to_digit(16).expect("valid low hex nibble") as u8;
    (hi << 4) | lo
}
