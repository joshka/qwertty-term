//! Generative differential sweep: build structured-random VT streams from a
//! vocabulary of well-defined *state-changing* sequences and assert the pure-
//! Rust engine and the Zig reference reach identical screen text + cursor.
//!
//! Deterministic (fixed seeds), so a divergence is reproducible from its printed
//! input. Only screen text + cursor are compared — never reply bytes — so the
//! known termio-layer reply divergences (DSR/DA/DECRQSS/XTGETTCAP/color queries)
//! are out of scope by construction.
//!
//! `cargo test -p vt-diff --features reference --test generative_sweep`

#![cfg(feature = "reference")]

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal};

/// Tiny deterministic xorshift64* PRNG — reproducible, no external deps.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n`.
    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
    /// Small param `1..=max`.
    fn param(&mut self, max: u32) -> u32 {
        1 + self.below(max)
    }
}

/// Append one random well-defined operation to `out`, given the grid size (used
/// to bias coordinates toward — and just past — the edges).
fn gen_op(rng: &mut Rng, out: &mut Vec<u8>, cols: u16, rows: u16) {
    // Weighted by the roll below; printing dominates so the grid fills.
    match rng.below(100) {
        // --- printing (0..40): mostly ASCII, some wide/combining UTF-8 ---
        0..=33 => {
            let c = 0x21 + rng.below(0x5e) as u8; // printable ASCII
            out.push(c);
        }
        34..=37 => {
            // A 2-byte UTF-8 letter (é) and a wide CJK char, to exercise width.
            out.extend_from_slice(if rng.below(2) == 0 {
                "é".as_bytes()
            } else {
                "世".as_bytes()
            });
        }
        38..=39 => out.push(b' '),
        // --- C0 movement ---
        40..=43 => out.push(b'\r'),
        44..=47 => out.push(b'\n'),
        48..=49 => out.push(0x08), // BS
        50..=51 => out.push(0x09), // HT
        // --- cursor positioning (bias just past the edges to test clamping) ---
        52..=57 => {
            let r = rng.param(rows as u32 + 2);
            let c = rng.param(cols as u32 + 2);
            out.extend_from_slice(format!("\x1b[{r};{c}H").as_bytes());
        }
        58..=61 => {
            let n = rng.param(8);
            let dir = [b'A', b'B', b'C', b'D'][rng.below(4) as usize];
            out.extend_from_slice(format!("\x1b[{n}").as_bytes());
            out.push(dir);
        }
        62..=63 => {
            out.extend_from_slice(format!("\x1b[{}G", rng.param(cols as u32 + 2)).as_bytes())
        }
        64..=65 => {
            out.extend_from_slice(format!("\x1b[{}d", rng.param(rows as u32 + 2)).as_bytes())
        }
        // --- erase ---
        66..=68 => out.extend_from_slice(format!("\x1b[{}J", rng.below(3)).as_bytes()),
        69..=71 => out.extend_from_slice(format!("\x1b[{}K", rng.below(3)).as_bytes()),
        72..=73 => out.extend_from_slice(format!("\x1b[{}X", rng.param(cols as u32)).as_bytes()),
        // --- insert/delete ---
        74 => out.extend_from_slice(format!("\x1b[{}L", rng.param(4)).as_bytes()),
        75 => out.extend_from_slice(format!("\x1b[{}M", rng.param(4)).as_bytes()),
        76 => out.extend_from_slice(format!("\x1b[{}@", rng.param(4)).as_bytes()),
        77 => out.extend_from_slice(format!("\x1b[{}P", rng.param(4)).as_bytes()),
        // --- scroll region + origin/autowrap/insert modes ---
        78..=80 => {
            let t = rng.param(rows as u32);
            let b = rng.param(rows as u32);
            out.extend_from_slice(format!("\x1b[{t};{b}r").as_bytes());
        }
        81 => out.extend_from_slice(b"\x1b[?7h"), // autowrap on
        82 => out.extend_from_slice(b"\x1b[?7l"), // autowrap off
        83 => out.extend_from_slice(b"\x1b[?6h"), // origin on
        84 => out.extend_from_slice(b"\x1b[?6l"), // origin off
        85 => out.extend_from_slice(b"\x1b[4h"),  // insert mode on
        86 => out.extend_from_slice(b"\x1b[4l"),  // insert mode off
        // --- ESC single-byte movers + tab control ---
        87..=88 => out.extend_from_slice(b"\x1bM"), // RI
        89..=90 => out.extend_from_slice(b"\x1bD"), // IND
        91 => out.extend_from_slice(b"\x1bE"),      // NEL
        92 => out.extend_from_slice(b"\x1bH"),      // HTS
        93 => out.extend_from_slice(format!("\x1b[{}g", 3 * rng.below(2)).as_bytes()), // TBC 0/3
        // --- REP (repeat previous printable) ---
        94..=95 => out.extend_from_slice(format!("\x1b[{}b", rng.param(6)).as_bytes()),
        // --- alt screen enter/leave (well-defined) ---
        96 => out.extend_from_slice(b"\x1b[?1049h"),
        97 => out.extend_from_slice(b"\x1b[?1049l"),
        // --- SGR (parser exercise; no text/cursor effect) ---
        _ => {
            let n = [0u32, 1, 4, 7, 30, 31, 37, 40, 44, 47][rng.below(10) as usize];
            out.extend_from_slice(format!("\x1b[{n}m").as_bytes());
        }
    }
}

/// Render an input as a copy-pasteable escaped byte string for triage.
fn escape(input: &[u8]) -> String {
    let mut s = String::new();
    for &b in input {
        match b {
            0x1b => s.push_str("\\x1b"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            0x08 => s.push_str("\\x08"),
            0x09 => s.push_str("\\t"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

fn run_sweep(iterations: u32, grids: &[(u16, u16)]) -> Vec<(u64, u16, u16, Vec<u8>, String)> {
    let mut divergences = Vec::new();
    for iter in 0..iterations {
        // Distinct seed per (iteration, grid) so every case is reproducible.
        let seed = 0x9E37_79B9_7F4A_7C15u64
            .wrapping_mul(iter as u64 + 1)
            .wrapping_add(0xD1B5);
        let (cols, rows) = grids[iter as usize % grids.len()];
        let mut rng = Rng::new(seed);
        let op_count = 8 + rng.below(48);
        let mut input = Vec::new();
        for _ in 0..op_count {
            gen_op(&mut rng, &mut input, cols, rows);
        }

        let mut reference = ReferenceTerminal::new(cols, rows);
        let mut rust = RustTerminal::new(cols, rows);
        reference.feed(&input);
        rust.feed(&input);
        let rd = reference.dump();
        let ud = rust.dump();

        if rd.text != ud.text || rd.cursor != ud.cursor {
            let detail = format!(
                "cursor ref={:?} rust={:?}\n--- ref text ---\n{}\n--- rust text ---\n{}",
                rd.cursor, ud.cursor, rd.text, ud.text
            );
            divergences.push((seed, cols, rows, input, detail));
        }
    }
    divergences
}

#[test]
fn generative_sweep_agrees() {
    let grids = [(8u16, 4u16), (10, 6), (20, 5), (16, 8), (5, 3)];
    let divergences = run_sweep(20_000, &grids);

    if !divergences.is_empty() {
        // Report up to a handful, deduped is not needed at this scale — the
        // first few are enough to triage.
        let show = divergences.len().min(6);
        let mut msg = format!(
            "generative sweep found {} divergence(s); showing {}:\n",
            divergences.len(),
            show
        );
        for (seed, cols, rows, input, detail) in divergences.iter().take(show) {
            msg.push_str(&format!(
                "\n=== seed={seed:#x} grid={cols}x{rows} ===\ninput: {}\n{detail}\n",
                escape(input)
            ));
        }
        panic!("{msg}");
    }
}
