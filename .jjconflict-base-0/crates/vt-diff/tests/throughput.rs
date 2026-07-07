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
    let streams: [(&str, Vec<u8>); 4] = [
        ("ascii", ascii_stream()),
        ("sgr-heavy", sgr_stream()),
        ("utf8-mixed", utf8_stream()),
        ("cursor-heavy", cursor_heavy_stream()),
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
