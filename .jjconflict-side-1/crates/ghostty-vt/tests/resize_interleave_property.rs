//! Deterministic property test: pseudo-random byte/resize interleavings must
//! never panic ghostty-vt.
//!
//! This is the CI-friendly guard for the resize blind spot. The pure-bytes
//! fuzz campaign (7.9M runs) never resized the grid, so it could not have
//! found the release-only `Screen::cursor_absolute` panic produced by a larger
//! resize followed by an alt-screen (DEC 1049) enter and a far-corner CUP. The
//! `resize` cargo-fuzz target is the campaign tool; this test is the always-on
//! regression net.
//!
//! It runs a few thousand scripts from a set of FIXED seeds (no clock, no
//! runtime randomness — the PRNG is a hand-rolled SplitMix64 seeded from
//! hardcoded constants) so failures are perfectly reproducible. It is
//! deliberately meaningful in release too and is part of the
//! `cargo test -p ghostty-vt --release` gate, because the guarded crash only
//! manifested in release (`debug_assert!` bounds checks are compiled out).

use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

/// Minimal SplitMix64 PRNG. Deterministic, dependency-free, good enough to
/// spread bytes/dimensions across the interesting ranges. Not cryptographic.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

/// Dimension band for resizes: exercise both grow and shrink relative to the
/// 80x24 start without allocating pathological grids. Matches the fuzz
/// target's `1..=MAX_DIM` clamp.
const MAX_DIM: u16 = 500;

/// A pool of escape sequences that stress the resize/cursor/pin desync path.
/// The generator mixes these with random printable/wide bytes.
const ESC_TOKENS: &[&[u8]] = &[
    b"\x1b[?1049h",   // alt enter (save cursor + clear)
    b"\x1b[?1049l",   // alt exit (restore cursor)
    b"\x1b[?1047h",   // legacy alt enter
    b"\x1b[?1047l",   // legacy alt exit
    b"\x1b[?47h",     // oldest alt enter
    b"\x1b[?47l",     // oldest alt exit
    b"\x1b[999;999H", // CUP far corner (clamps)
    b"\x1b[H",        // CUP home
    b"\x1b[1;1H",
    b"\x1b[500;500H",
    b"\x1b[5;20r",   // DECSTBM scroll region
    b"\x1b[r",       // DECSTBM reset
    b"\x1b[10;60s",  // DECSLRM margins
    b"\x1b[?6h",     // origin mode on
    b"\x1b[?6l",     // origin mode off
    b"\x1b[?7h",     // wraparound on
    b"\x1b[?7l",     // wraparound off
    b"\x1b[?69h",    // left/right margin mode on
    b"\x1b[?69l",    // left/right margin mode off
    b"\x1b[2J",      // erase display
    b"\x1b[3S",      // scroll up
    b"\x1b[3T",      // scroll down
    b"\r\n",         // CRLF
    "あ".as_bytes(), // a wide CJK glyph
    "🦀".as_bytes(), // an emoji (wide, multi-byte)
    b"A",            // a narrow ASCII glyph
];

/// Build one pseudo-random script buffer of interleaved bytes and, mixed in,
/// occasional resize markers. We encode the script directly as a Vec of
/// operations rather than the fuzz wire format, since here we drive the
/// terminal in-process.
enum Op {
    Feed(Vec<u8>),
    Resize(u16, u16),
}

fn gen_script(rng: &mut SplitMix64, max_ops: u32) -> Vec<Op> {
    let n_ops = 1 + rng.below(max_ops);
    let mut ops = Vec::with_capacity(n_ops as usize);
    for _ in 0..n_ops {
        // ~1 in 4 ops is a resize; the rest are byte feeds.
        if rng.below(4) == 0 {
            let cols = (rng.below(MAX_DIM as u32) as u16).max(1);
            let rows = (rng.below(MAX_DIM as u32) as u16).max(1);
            ops.push(Op::Resize(cols, rows));
        } else {
            // A feed chunk is a small run of tokens/bytes.
            let chunk_tokens = 1 + rng.below(6);
            let mut buf = Vec::new();
            for _ in 0..chunk_tokens {
                if rng.below(2) == 0 {
                    // A known escape token.
                    let tok = ESC_TOKENS[rng.below(ESC_TOKENS.len() as u32) as usize];
                    buf.extend_from_slice(tok);
                } else {
                    // A raw byte (covers control bytes, high bytes, split
                    // UTF-8 sequences the decoder must survive).
                    buf.push(rng.below(256) as u8);
                }
            }
            ops.push(Op::Feed(buf));
        }
    }
    ops
}

/// Drive one script against a fresh terminal. The whole point is that this
/// returns normally (no panic) for every generated script.
fn run(ops: &[Op]) {
    let term = Terminal::new(Options {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    for op in ops {
        match op {
            Op::Feed(bytes) => stream.feed(bytes),
            Op::Resize(cols, rows) => stream.handler.terminal.resize(*cols, *rows),
        }
    }
    // Touch cursor state so any latent desync surfaces as a panic here.
    std::hint::black_box(stream.handler.terminal.screen().cursor.x);
    std::hint::black_box(stream.handler.terminal.screen().cursor.y);
}

/// Run `count` scripts from a fixed seed. Panics inside `run` fail the test.
fn campaign(seed: u64, count: u32, max_ops: u32) {
    let mut rng = SplitMix64(seed);
    for _ in 0..count {
        let ops = gen_script(&mut rng, max_ops);
        run(&ops);
    }
}

#[test]
fn interleaved_resize_never_panics_seed_a() {
    // Fixed seed; short scripts, many iterations — broad coverage of the
    // per-op resize/cursor-remap path.
    campaign(0x1234_5678_9abc_def0, 5000, 24);
}

#[test]
fn interleaved_resize_never_panics_seed_b() {
    // A different fixed seed with longer scripts — deeper histories where a
    // resize lands after substantial alt-screen churn.
    campaign(0x0fed_cba9_8765_4321, 2000, 80);
}

#[test]
fn interleaved_resize_never_panics_seed_c() {
    // A third seed biased (via a small dimension band through more ops) toward
    // rapid alternate grid sizes.
    campaign(0xdead_beef_cafe_f00d, 3000, 40);
}

/// The exact field-crash shape, exhaustively over a grid of resize dimensions:
/// resize to (cols, rows), enter alt via 1049, CUP to the far corner. This is
/// the specific interleaving the campaign missed; sweep it so no dimension
/// combination regresses.
#[test]
fn field_crash_shape_over_dimension_grid() {
    for &cols in &[1u16, 2, 10, 79, 80, 81, 120, 200, 500] {
        for &rows in &[1u16, 2, 10, 23, 24, 25, 40, 60, 500] {
            let term = Terminal::new(Options {
                cols: 80,
                rows: 24,
                ..Default::default()
            });
            let mut s = Stream::new(TerminalHandler::new(term));
            s.feed(b"primary text");
            s.handler.terminal.resize(cols, rows);
            s.feed(b"\x1b[?1049h");
            s.feed(b"\x1b[999;999H");
            s.feed(b"alt corner\r\n");
            // Shrink/grow again while on alt, then exit.
            s.handler.terminal.resize(rows, cols); // swap dims
            s.feed(b"\x1b[999;999H");
            s.feed(b"\x1b[?1049l");
            s.feed(b"back");
            std::hint::black_box(s.handler.terminal.screen().cursor.x);
        }
    }
}
