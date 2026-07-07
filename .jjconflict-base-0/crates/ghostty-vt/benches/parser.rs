//! Criterion bench skeleton for the VT parser.
//!
//! Feeds representative byte streams through [`Parser::next`] and drains all
//! three action slots so the optimizer cannot elide the work. Three streams
//! mirror the rewrite-prompt's Phase-1 throughput targets:
//!
//! - plain ASCII (ground-state print fast path),
//! - SGR-heavy (CSI param/colon accumulation + dispatch),
//! - mixed UTF-8 (drives the parser via the UTF-8 decoder, as the stream
//!   layer will).
//!
//! Skeleton only: the goal is a stable harness plus baseline numbers, not a
//! tuned benchmark. Run with `cargo bench -p ghostty-vt`.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ghostty_vt::parser::Parser;
use ghostty_vt::utf8_decoder::Utf8Decoder;

/// ~4 KiB of printable ASCII.
fn ascii_stream() -> Vec<u8> {
    let line = b"The quick brown fox jumps over the lazy dog. 0123456789.\r\n";
    line.iter().copied().cycle().take(4096).collect()
}

/// SGR-heavy stream: truecolor fg/bg with colon subparams, repeated.
fn sgr_stream() -> Vec<u8> {
    let seq = b"\x1b[38:2:175:175:215;48:2:0:0:0;1;4:3mX\x1b[0m";
    seq.iter().copied().cycle().take(4096).collect()
}

/// Mixed UTF-8 stream: ASCII text interleaved with multi-byte scalars and a
/// few control sequences.
fn utf8_stream() -> Vec<u8> {
    let mut out = Vec::new();
    let sample = "Héllo, 世界! 😄 café — naïve \x1b[1mbold\x1b[0m ✤\r\n";
    while out.len() < 4096 {
        out.extend_from_slice(sample.as_bytes());
    }
    out
}

/// Drive raw bytes straight through the parser (no UTF-8 decoding). Matches
/// how the stream layer feeds the parser while a control sequence is in
/// flight.
fn drive_parser(bytes: &[u8]) {
    let mut parser = Parser::new();
    for &b in bytes {
        for action in parser.next(b) {
            black_box(&action);
        }
    }
}

/// Drive bytes through the UTF-8 decoder first (ground state), forwarding
/// decoded codepoints and control bytes to the parser the way the stream
/// layer will. Approximation: `<= 0x1F` and decoded control codepoints go to
/// the parser as raw bytes; everything else is a print we simply consume.
fn drive_decoder_and_parser(bytes: &[u8]) {
    let mut decoder = Utf8Decoder::new();
    let mut parser = Parser::new();
    for &b in bytes {
        // Re-feed on non-consumed (ill-formed) bytes, per the decoder contract.
        let mut consumed = false;
        while !consumed {
            let (cp, c) = decoder.next(b);
            consumed = c;
            if let Some(cp) = cp {
                if (cp as u32) <= 0x1F {
                    for action in parser.next(cp as u32 as u8) {
                        black_box(&action);
                    }
                } else {
                    black_box(cp);
                }
            }
        }
    }
}

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser");

    let ascii = ascii_stream();
    group.throughput(Throughput::Bytes(ascii.len() as u64));
    group.bench_function("ascii", |b| b.iter(|| drive_parser(black_box(&ascii))));

    let sgr = sgr_stream();
    group.throughput(Throughput::Bytes(sgr.len() as u64));
    group.bench_function("sgr", |b| b.iter(|| drive_parser(black_box(&sgr))));

    let utf8 = utf8_stream();
    group.throughput(Throughput::Bytes(utf8.len() as u64));
    group.bench_function("utf8_mixed", |b| {
        b.iter(|| drive_decoder_and_parser(black_box(&utf8)))
    });

    group.finish();
}

criterion_group!(benches, bench_parser);
criterion_main!(benches);
