//! Bold/italic acceptance test: STYLED PIXELS.
//!
//! Drives a real `qwertty_term_vt::Terminal` through a `Stream` with the field
//! reproduction line `\x1b[1mBOLD\x1b[0m plain \x1b[3mital\x1b[0m`, snapshots
//! it, builds cell buffers via the cell engine (which resolves each cell's
//! bold/italic attributes to a styled face in the completed style table and
//! rasterizes through it), draws an offscreen frame, and reads the pixels back.
//!
//! It asserts the load-bearing rendering facts the "text feels thin" field
//! report was about:
//!
//! - The **bold** span has measurably HIGHER ink coverage than the same letters
//!   rendered regular (bold really is heavier, not just re-using the regular
//!   face).
//! - The **italic** span differs pixel-wise from the same letters rendered
//!   regular (the italic face's skewed outlines land on different pixels).
//!
//! Dumps a specimen PNG artifact for human inspection.
//!
//! Runs over the platform-free [`Software`] backend (ADR 003), so — like
//! `software_headless.rs` — it **never skips** and is **not OS-gated**: it runs
//! on macOS (over the CoreText face) and on Linux CI (over the FreeType face)
//! alike (`Face` is the cfg-selected platform alias). The synthetic-bold /
//! italic facts it asserts are properties of the face rasterizer + CPU
//! compositor, both backend-agnostic. (#42 — real Linux pixel coverage beyond
//! the headless smoke test.)

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Face, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_renderer::software::Software;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

/// One rendered pixel, unpacked from the BGRA readback buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

/// A read-back frame: BGRA pixels + dimensions, with cell-aware sampling.
struct Frame {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    cell_w: usize,
    cell_h: usize,
}

impl Frame {
    fn px(&self, x: usize, y: usize) -> Px {
        let i = (y * self.width + x) * 4;
        Px {
            b: self.pixels[i],
            g: self.pixels[i + 1],
            r: self.pixels[i + 2],
            a: self.pixels[i + 3],
        }
    }

    /// Total ink coverage over a cell: sum over the cell's pixels of the
    /// per-pixel coverage vs the background (max abs channel delta). A heavier
    /// (bolder) glyph darkens more pixels more, so its coverage sum is larger.
    fn cell_ink(&self, col: usize, row: usize, bg: Px) -> i64 {
        let mut sum = 0i64;
        for dy in 0..self.cell_h {
            for dx in 0..self.cell_w {
                let x = col * self.cell_w + dx;
                let y = row * self.cell_h + dy;
                if x >= self.width || y >= self.height {
                    continue;
                }
                let p = self.px(x, y);
                let d = (p.r as i32 - bg.r as i32)
                    .abs()
                    .max((p.g as i32 - bg.g as i32).abs())
                    .max((p.b as i32 - bg.b as i32).abs());
                sum += d as i64;
            }
        }
        sum
    }
}

/// Per-pixel coverage vs background (max abs channel delta).
fn cov(p: Px, bg: Px) -> i32 {
    (p.r as i32 - bg.r as i32)
        .abs()
        .max((p.g as i32 - bg.g as i32).abs())
        .max((p.b as i32 - bg.b as i32).abs())
}

/// Build a `Grid` with the full default style table (Regular / Bold / Italic /
/// BoldItalic from the embedded variable fonts), mirroring the app's default
/// (no-font-family) grid construction.
fn make_grid(face: Face, size_px: f64) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let collection =
        Collection::new_with_default_fallbacks(face, size_px).expect("default style table");
    let resolver = CodepointResolver::new(collection);
    Grid::new(resolver, metrics).expect("grid")
}

/// Render a line through a fresh grid over the Software backend and read pixels
/// back.
fn render_line(backend: Software, size_px: f64, line: &str) -> (Frame, Px) {
    let face = Face::load_embedded(size_px).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(face, size_px);

    let term = Terminal::new(Options {
        cols: 40,
        rows: 1,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(line.as_bytes());
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions {
        cursor_blink_visible: false,
        ..FrameOptions::default()
    };
    engine.update_frame(&snapshot, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");
    let (sw, sh) = engine.screen_size();
    let bg = Px {
        r: opts.default_bg.r,
        g: opts.default_bg.g,
        b: opts.default_bg.b,
        a: 255,
    };
    (
        Frame {
            pixels,
            width: sw,
            height: sh,
            cell_w: cw as usize,
            cell_h: ch as usize,
        },
        bg,
    )
}

#[test]
fn bold_italic_pixels_offscreen_readback() {
    let size_px = 32.0; // larger cell => more pixels to measure the weight delta

    // The field reproduction line: BOLD (SGR 1), then plain, then italic (SGR 3).
    // "plainn" (6 chars) pads so the italic span "ital" lands clear of the bold.
    let styled_line = "\x1b[1mBOLD\x1b[0m plain \x1b[3mital\x1b[0m";
    // The same visible text with NO styling — the regular-weight baseline.
    let plain_line = "BOLD plain ital";

    // Column layout is identical between the two lines (styling is zero-width),
    // so a column index means the same character in both frames.
    //   cols 0..=3  : B O L D   (bold in styled, regular in plain)
    //   col  4      : space
    //   cols 5..=9  : p l a i n
    //   col  10     : space
    //   cols 11..=14: i t a l   (italic in styled, regular in plain)
    let bold_cols = [0usize, 1, 2, 3];
    let ital_cols = [11usize, 12, 13, 14];

    let (styled, bg) = render_line(Software::new(), size_px, styled_line);
    let (plain, _) = render_line(Software::new(), size_px, plain_line);

    // === ASSERTION 1: the BOLD span has higher ink coverage than the same
    // letters rendered regular. ===
    let bold_ink: i64 = bold_cols.iter().map(|&c| styled.cell_ink(c, 0, bg)).sum();
    let regular_ink: i64 = bold_cols.iter().map(|&c| plain.cell_ink(c, 0, bg)).sum();
    assert!(
        regular_ink > 0,
        "baseline 'BOLD' letters should have ink (regular {regular_ink})"
    );
    assert!(
        bold_ink > regular_ink,
        "bold 'BOLD' ink coverage {bold_ink} should exceed regular {regular_ink} \
         (ratio {:.3})",
        bold_ink as f64 / regular_ink as f64
    );

    // === ASSERTION 2: the ITALIC span differs pixel-wise from the same letters
    // rendered regular (skewed outlines land on different pixels). Compare the
    // styled-italic cell against the plain-regular cell at the same column. ===
    let ital_vs_regular: i64 = ital_cols
        .iter()
        .map(|&c| cell_cross_diff(&styled, &plain, c, 0, bg))
        .sum();
    // Sanity: the italic cells have ink to compare.
    let ital_ink: i64 = ital_cols.iter().map(|&c| styled.cell_ink(c, 0, bg)).sum();
    assert!(ital_ink > 0, "italic 'ital' letters should have ink");
    assert!(
        ital_vs_regular > 0,
        "italic 'ital' should differ pixel-wise from regular; diff {ital_vs_regular}"
    );

    // Report the numbers.
    println!(
        "bold-italic coverage: bold_ink={bold_ink} regular_ink={regular_ink} \
         ratio={:.3}; italic_vs_regular_diff={ital_vs_regular} italic_ink={ital_ink}",
        bold_ink as f64 / regular_ink as f64
    );

    // === Specimen PNG for human inspection. ===
    if let Some(path) = dump_png(&styled, "bold-italic-pixels.png") {
        println!("bold-italic specimen written to {path}");
    }
}

/// Cross-frame per-cell coverage diff: sum of abs coverage differences between
/// the same cell in two frames (same geometry).
fn cell_cross_diff(a: &Frame, b: &Frame, col: usize, row: usize, bg: Px) -> i64 {
    let mut sum = 0i64;
    for dy in 0..a.cell_h {
        for dx in 0..a.cell_w {
            let x = col * a.cell_w + dx;
            let y = row * a.cell_h + dy;
            let ca = if x < a.width && y < a.height {
                cov(a.px(x, y), bg)
            } else {
                0
            };
            let cb = if x < b.width && y < b.height {
                cov(b.px(x, y), bg)
            } else {
                0
            };
            sum += (ca - cb).abs() as i64;
        }
    }
    sum
}

/// Write the BGRA frame to `target/<name>` as an RGBA PNG. Returns the path.
fn dump_png(frame: &Frame, name: &str) -> Option<String> {
    let path = format!("{}/../../target/{name}", env!("CARGO_MANIFEST_DIR"));
    let mut rgba = Vec::with_capacity(frame.width * frame.height * 4);
    for chunk in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    let bytes = encode_png(frame.width as u32, frame.height as u32, &rgba)?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Minimal PNG encoder: 8-bit RGBA, stored (uncompressed) zlib blocks.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (!(crc & 1)).wrapping_add(1);
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }
    fn adler32(bytes: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in bytes {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc_input);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        let start = y * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }

    let mut zlib = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let block = &raw[pos..(pos + 65535).min(raw.len())];
        let last = pos + block.len() >= raw.len();
        zlib.push(if last { 1 } else { 0 });
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
        pos += block.len();
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}
