//! Differential test: pure-Rust `qwertty-term-vt` vs the Zig `libghostty-vt`
//! reference, comparing screen text + cursor position.
//!
//! Only runs with the `reference` feature (links the Zig-built static lib):
//! `cargo test -p vt-diff --features reference`.

#![cfg(feature = "reference")]

use std::fs;
use std::path::Path;

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal, ScreenDump, decode_escaped_stream};

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
fn hand_scroll_region_fast_path() {
    // Exercises the in-place region-scroll fast path (cursor_scroll_region_up):
    // a top != 0, full-width, zero-blank region scrolled many times. Wide (CJK)
    // chars are included because a wide-spacer-head at the region boundary was
    // the exact class that diverged while porting upstream's cursorScrollRegionUp
    // (77190bd02) — the fast path is deliberately restricted to a zero blank so
    // its clear+rotate is bit-identical to the erase_row_bounded path.
    assert_agree(
        "scroll_region_fast_ascii",
        10,
        5,
        b"\x1B[2;4rtop\x1B[2;1HA\r\nB\r\nC\r\nD\r\nE\r\nF\r\nG",
    );
    // Wide chars filling to the last column (forces spacer heads) then scroll.
    assert_agree(
        "scroll_region_fast_wide",
        5,
        4,
        "\x1B[2;4r\u{4E16}\u{4E16}\u{4E16}\r\n\u{4E16}\u{4E16}\u{4E16}\r\n\u{4E16}\u{4E16}\u{4E16}"
            .as_bytes(),
    );
    // Deep region so the region spans a page boundary (slow path) on a small
    // grid with a lot of scrolling.
    assert_agree(
        "scroll_region_fast_deep",
        8,
        6,
        b"\x1B[1;6r\x1B[6;1H0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9",
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

// ---- resize + alt-screen regression (field crash) ----------------------

/// Feed `pre`, resize both engines to `cols2`x`rows2`, feed `post`, and assert
/// identical observable state. Exercises the mid-stream resize path the corpus
/// format cannot express.
fn assert_agree_resize(
    label: &str,
    cols1: u16,
    rows1: u16,
    pre: &[u8],
    cols2: u16,
    rows2: u16,
    post: &[u8],
) {
    let mut reference = ReferenceTerminal::new(cols1, rows1);
    let mut rust = RustTerminal::new(cols1, rows1);
    reference.feed(pre);
    rust.feed(pre);
    reference.resize(cols2, rows2);
    rust.resize(cols2, rows2);
    reference.feed(post);
    rust.feed(post);

    let rd = reference.dump();
    let ud = rust.dump();
    assert_eq!(rd.text, ud.text, "TEXT diverged for `{label}`");
    assert_eq!(rd.cursor, ud.cursor, "CURSOR diverged for `{label}`");
}

/// Field crash: resize the terminal larger (window-appearance), then enter the
/// alternate screen and move the cursor to the far corner. Before the fix the
/// Rust port panicked in `cursor_absolute` in release builds because the
/// alternate screen was created at the stale construction-time size. Now it must
/// match the Zig oracle.
#[test]
fn resize_then_alt_screen_field_crash() {
    assert_agree_resize(
        "resize_then_alt_screen",
        80,
        24,
        b"",
        120,
        40,
        b"\x1B[?1049h\x1B[999;999Hhello from the alt screen",
    );
}

/// Same class, with primary content and a deep cursor before the switch.
#[test]
fn resize_deep_cursor_then_alt_screen() {
    assert_agree_resize(
        "resize_deep_cursor_alt",
        80,
        24,
        b"top line\r\nsecond line\r\nthird line",
        100,
        50,
        b"\x1B[45;90Hdeep\x1B[?1049halt\x1B[?1049lback",
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
