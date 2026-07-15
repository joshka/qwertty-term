//! Profiling driver: replays synthesized vtebench-equivalent payloads through
//! the pure-Rust `qwertty-term-vt` engine in a tight loop so a sampler (samply /
//! cargo flamegraph) attributes time to the parser / print / decode paths.
//!
//! Usage:
//!   cargo build -p vt-diff --release --example profile_streams
//!   samply record ./target/release/examples/profile_streams <stream> <iters>
//!
//! `<stream>` is one of: ascii sgr utf8 cursor dense erase redraw cjk scrolling scroll-region all
//! `<iters>` repeats the payload feed that many times (default sized for ~seconds).

use std::time::Instant;

use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

const COLS: u16 = 120;
const ROWS: u16 = 40;
const STREAM_MIB: usize = 8;

/// Terminal geometry, overridable via `COLS`/`ROWS` env vars so a payload can
/// be measured at another size (e.g. 80x24 to match the upstream bench).
fn geom() -> (u16, u16) {
    let g = |k: &str, d: u16| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    (g("COLS", COLS), g("ROWS", ROWS))
}

fn ascii_stream() -> Vec<u8> {
    let line = "The quick brown fox jumps over the lazy dog 0123456789 !@#$%^&*()_+-=[]{};:'\r\n";
    line.bytes()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn sgr_stream() -> Vec<u8> {
    let chunk = "\x1b[1;31mred\x1b[0m \x1b[38;5;120mpal\x1b[0m \x1b[38;2;10;20;30mrgb\x1b[0m \x1b[4:3m~\x1b[0m\r\n";
    chunk
        .bytes()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn utf8_stream() -> Vec<u8> {
    let chunk = "héllo wörld 好的 テスト 🙂👍 mixed ascii tail\r\n";
    chunk
        .bytes()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

/// Wide-heavy stream: mostly CJK (width-2) codepoints, exercising the wide
/// print_slice fill's (wide, spacer_tail) pair path.
fn cjk_stream() -> Vec<u8> {
    let chunk = "你好世界这是宽字符吞吐测试文本一二三四五六七八九十百千万\r\n";
    chunk
        .bytes()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn cursor_stream() -> Vec<u8> {
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

fn dense_cells_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 4096);
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut offset: u32 = 0;
    let mut li = 0usize;
    'outer: loop {
        let ch = letters[li % letters.len()] as char;
        li += 1;
        v.extend_from_slice(b"\x1b[H");
        for line in 1..=ROWS as u32 {
            for column in 1..=COLS as u32 {
                let index = line + column + offset;
                let fg_col = index % 156 + 100;
                let bg_col = 255 - index % 156 + 100;
                v.extend_from_slice(
                    format!("\x1b[38;5;{fg_col};48;5;{bg_col};1;3;4m{ch}").as_bytes(),
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

/// Upstream's "styled paint + ED 2" pattern (8d663a76e): paint a full screen
/// of styled rows, erase it with ED 2, repeat. Exercises the clear_cells
/// style-ref release path the way full-screen TUIs do on clear/redraw.
fn erase_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 4096);
    let mut color: u32 = 0;
    'outer: loop {
        v.extend_from_slice(b"\x1b[H");
        for _line in 1..=ROWS as u32 {
            // One style per row, full row of text: styled cells arrive in
            // long same-style runs, the common TUI shape.
            v.extend_from_slice(format!("\x1b[38;5;{}m", color % 156 + 100).as_bytes());
            v.extend(std::iter::repeat_n(b'x', COLS as usize));
            v.extend_from_slice(b"\r\n");
            color += 1;
            if v.len() >= STREAM_MIB * 1024 * 1024 {
                break 'outer;
            }
        }
        v.extend_from_slice(b"\x1b[2J");
    }
    v
}

/// Upstream's "TUI redraw" pattern (cb2d78587): repaint the full screen with
/// uniform-style rows whose color rotates every frame, no clear in between —
/// every cell is overwritten with a different style than it holds. Exercises
/// the print_slice bulk style-only fill.
fn redraw_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 4096);
    let mut frame: u32 = 0;
    'outer: loop {
        v.extend_from_slice(b"\x1b[H");
        for line in 0..ROWS as u32 {
            v.extend_from_slice(format!("\x1b[38;5;{}m", (frame + line) % 156 + 100).as_bytes());
            v.extend(std::iter::repeat_n(b'x', COLS as usize));
            v.extend_from_slice(b"\r\n");
            if v.len() >= STREAM_MIB * 1024 * 1024 {
                break 'outer;
            }
        }
        frame += 1;
    }
    v
}

fn scrolling_stream() -> Vec<u8> {
    b"y\n"
        .iter()
        .copied()
        .cycle()
        .take(STREAM_MIB * 1024 * 1024)
        .collect()
}

fn scroll_region_stream() -> Vec<u8> {
    let mut v = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + 16);
    v.extend_from_slice(format!("\x1b[5;{}r", ROWS - 4).as_bytes());
    v.extend_from_slice(b"\x1b[6;1H");
    while v.len() < STREAM_MIB * 1024 * 1024 {
        v.extend_from_slice(b"y\n");
    }
    v
}

fn feed(stream: &[u8], iters: usize) {
    for _ in 0..iters {
        let (cols, rows) = geom();
        let terminal = Terminal::new(Options {
            cols,
            rows,
            max_scrollback: 0,
            colors: Default::default(),
        });
        let mut s = Stream::new(TerminalHandler::new(terminal));
        for chunk in stream.chunks(64 * 1024) {
            s.feed(chunk);
        }
        std::hint::black_box(&s.handler.terminal);
    }
}

/// A handler that decodes + dispatches but does NO terminal print work, so the
/// full-vs-noop delta attributes the print cost separately from decode/dispatch.
#[derive(Default)]
struct NoopHandler {
    sink: u64,
}
impl qwertty_term_vt::stream::Handler for NoopHandler {
    fn print(&mut self, cp: u32) {
        self.sink = self.sink.wrapping_add(cp as u64);
    }
    fn print_slice(&mut self, cps: &[u32]) {
        for &cp in cps {
            self.sink = self.sink.wrapping_add(cp as u64);
        }
    }
}

fn feed_noop(stream: &[u8], iters: usize) {
    for _ in 0..iters {
        let mut s = Stream::new(NoopHandler::default());
        for chunk in stream.chunks(64 * 1024) {
            s.feed(chunk);
        }
        std::hint::black_box(&s.handler.sink);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let which = args.get(1).map(String::as_str).unwrap_or("all");
    let iters: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);

    // `file:<path>` reads a raw payload and tiles it up to ~STREAM_MIB so a
    // small vtebench data file (e.g. benchmarks/unicode/symbols) is measured at
    // steady state through the engine.
    if let Some(path) = which.strip_prefix("file:") {
        let raw = std::fs::read(path).expect("read payload file");
        let mut stream = Vec::with_capacity(STREAM_MIB * 1024 * 1024 + raw.len());
        while stream.len() < STREAM_MIB * 1024 * 1024 {
            stream.extend_from_slice(&raw);
        }
        let noop = std::env::var("NOOP").is_ok();
        let start = Instant::now();
        if noop {
            feed_noop(&stream, iters);
        } else {
            feed(&stream, iters);
        }
        let secs = start.elapsed().as_secs_f64();
        let mib = (stream.len() * iters) as f64 / (1024.0 * 1024.0);
        let tag = if noop { "noop" } else { "full" };
        eprintln!(
            "{path:<28} {tag} {:>8.1} MiB/s  ({iters} iters)",
            mib / secs
        );
        return;
    }

    let all: Vec<(&str, Vec<u8>)> = vec![
        ("ascii", ascii_stream()),
        ("sgr", sgr_stream()),
        ("utf8", utf8_stream()),
        ("cjk", cjk_stream()),
        ("cursor", cursor_stream()),
        ("dense", dense_cells_stream()),
        ("erase", erase_stream()),
        ("redraw", redraw_stream()),
        ("scrolling", scrolling_stream()),
        ("scroll-region", scroll_region_stream()),
    ];

    for (name, stream) in &all {
        if which != "all" && which != *name {
            continue;
        }
        let noop = std::env::var("NOOP").is_ok();
        let start = Instant::now();
        if noop {
            feed_noop(stream, iters);
        } else {
            feed(stream, iters);
        }
        let secs = start.elapsed().as_secs_f64();
        let mib = (stream.len() * iters) as f64 / (1024.0 * 1024.0);
        let tag = if noop { "noop" } else { "full" };
        eprintln!(
            "{name:<14} {tag} {:>8.1} MiB/s  ({iters} iters)",
            mib / secs
        );
    }
}
