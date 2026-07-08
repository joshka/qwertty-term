//! Head-to-head throughput comparison: RustTerminal vs the libghostty-vt
//! reference. Not a pass/fail test — prints MiB/s for each engine per stream.
//! The Phase-1 gate (rewrite prompt) is "within 2x of ghostty".
//!
//! Run explicitly, release mode, against a ReleaseFast-built reference lib:
//!   cargo test -p vt-diff --features reference --release -- --ignored --nocapture throughput
#![cfg(feature = "reference")]

use std::time::Instant;

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal};

const COLS: u16 = 120;
const ROWS: u16 = 40;
const STREAM_MIB: usize = 8;

fn ascii_stream() -> Vec<u8> {
    let line = "The quick brown fox jumps over the lazy dog 0123456789 !@#$%^&*()_+-=[]{};:'\r\n";
    line.as_bytes()
        .iter()
        .copied()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn sgr_stream() -> Vec<u8> {
    let chunk = "\x1b[1;31mred\x1b[0m \x1b[38;5;120mpal\x1b[0m \x1b[38;2;10;20;30mrgb\x1b[0m \x1b[4:3m~\x1b[0m\r\n";
    chunk
        .as_bytes()
        .iter()
        .copied()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn utf8_stream() -> Vec<u8> {
    let chunk = "héllo wörld 好的 テスト 🙂👍 mixed ascii tail\r\n";
    chunk
        .as_bytes()
        .iter()
        .copied()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn cursor_heavy_stream() -> Vec<u8> {
    // Simulates full-screen-app behavior: absolute moves + short prints + erases.
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 64);
    let mut row = 1u32;
    let mut col = 1u32;
    while v.len() < STREAM_MIB * 1024 * 1024 {
        v.extend_from_slice(format!("\x1b[{row};{col}Hcell\x1b[K").as_bytes());
        row = row % ROWS as u32 + 1;
        col = (col + 7) % (COLS as u32 - 8) + 1;
    }
    v
}

/// Faithful replica of vtebench's `dense_cells` payload
/// (`benchmarks/dense_cells/benchmark`): move home, then for every cell write a
/// fresh 256-color fg/bg + bold/italic/underline SGR followed by one printable
/// letter. This is the pure cell-write path — the one suite the whole-app
/// vtebench baseline loses to real Ghostty. Cursor home per full-grid pass.
fn dense_cells_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 4096);
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut offset: u32 = 0;
    let mut li = 0usize;
    'outer: loop {
        let char = letters[li % letters.len()] as char;
        li += 1;
        v.extend_from_slice(b"\x1b[H");
        for line in 1..=ROWS as u32 {
            for column in 1..=COLS as u32 {
                let index = line + column + offset;
                let fg_col = index % 156 + 100;
                let bg_col = 255 - index % 156 + 100;
                v.extend_from_slice(
                    format!("\x1b[38;5;{fg_col};48;5;{bg_col};1;3;4m{char}").as_bytes(),
                );
                if v.len() >= STREAM_MIB * 1024 * 1024 {
                    break 'outer;
                }
            }
        }
        offset += 1;
    }
    v
}

/// Faithful replica of vtebench's `scrolling` payload: `printf "y\n"` repeated.
/// A single printable char plus newline forces a scroll on every line once the
/// grid fills — exercises the scroll-up / linefeed path.
fn scrolling_stream() -> Vec<u8> {
    b"y\n"
        .iter()
        .copied()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

/// Scrolling confined to a DECSTBM region (mirrors vtebench's
/// `scrolling_*_region` setups, which set a top/bottom margin before scrolling).
/// Set once at the front, then the same `y\n` scroll payload.
fn scrolling_region_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 16);
    // DECSTBM: rows 5..=ROWS-4 as the scroll region.
    v.extend_from_slice(format!("\x1b[5;{}r", ROWS - 4).as_bytes());
    v.extend_from_slice(b"\x1b[6;1H");
    while v.len() < STREAM_MIB * 1024 * 1024 {
        v.extend_from_slice(b"y\n");
    }
    v
}

fn run<O: Oracle>(oracle: &mut O, stream: &[u8]) -> f64 {
    let start = Instant::now();
    // feed in 64 KiB chunks to mimic PTY read granularity
    for chunk in stream.chunks(64 * 1024) {
        oracle.feed(chunk);
    }
    let secs = start.elapsed().as_secs_f64();
    (stream.len() as f64 / (1024.0 * 1024.0)) / secs
}

#[test]
#[ignore = "benchmark: run explicitly in release mode"]
fn throughput() {
    let streams: [(&str, Vec<u8>); 7] = [
        ("ascii", ascii_stream()),
        ("sgr-heavy", sgr_stream()),
        ("utf8-mixed", utf8_stream()),
        ("cursor-heavy", cursor_heavy_stream()),
        ("dense_cells", dense_cells_stream()),
        ("scrolling", scrolling_stream()),
        ("scroll-region", scrolling_region_stream()),
    ];

    println!(
        "\n{:<14} {:>12} {:>12} {:>8}",
        "stream", "rust MiB/s", "ref MiB/s", "ratio"
    );
    for (name, stream) in &streams {
        let mut rust = RustTerminal::new(COLS, ROWS);
        let rust_rate = run(&mut rust, stream);
        let mut reference = ReferenceTerminal::new(COLS, ROWS);
        let ref_rate = run(&mut reference, stream);
        println!(
            "{:<14} {:>12.1} {:>12.1} {:>7.2}x",
            name,
            rust_rate,
            ref_rate,
            rust_rate / ref_rate
        );
        // sanity: engines still agree after the pounding
        assert_eq!(rust.text(), reference.text(), "divergence on {name}");
    }
}
