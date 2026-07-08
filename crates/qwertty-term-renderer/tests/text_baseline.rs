//! Glyph-baseline placement audit: is text ink placed at the font baseline the
//! cell metrics dictate, or is it vertically offset (the "bar cursor looks high
//! relative to the prompt text" field report)?
//!
//! A prior audit (`cursor_alignment.rs`) proved cursor-rect == cell-box exactly.
//! That leaves *text placement within the cell* as the live suspect: if font
//! glyphs render high/low of where upstream puts them while sprites (the cursor,
//! box drawing) are cell-exact, a correctly-boxed cursor looks misaligned
//! against the text.
//!
//! Ground truth (upstream `src/font/face/coretext.zig` `renderGlyph`, commit
//! `2da015cd6`): CoreText's `getBoundingRectsForGlyphs` returns ink bounds
//! relative to the glyph's **baseline** (the CoreGraphics drawing origin). Before
//! deriving the stored `offset_y`, upstream adds `metrics.cell_baseline`:
//!
//! ```zig
//! // We need to add the baseline position before passing to the constrain
//! // function since it operates on cell-relative positions, not baseline.
//! const cell_baseline: f64 = @floatFromInt(metrics.cell_baseline);
//! ... .y = rect.origin.y + cell_baseline ...
//! const offset_y: i32 = px_y + px_height;   // now cell-BOTTOM to ink-TOP
//! ```
//!
//! So the stored `offset_y` (== the shader's `bearings.y`) is the distance from
//! the **bottom of the cell** to the **top of the ink box**. The shader then
//! places the glyph top at `cell_size.y - bearings.y` from the cell top. With
//! the `+cell_baseline` term the 'H' cap sits at the correct cell-relative
//! height; without it, every font glyph renders exactly `cell_baseline` px too
//! low.
//!
//! This test renders baseline-revealing glyphs offscreen at two sizes and
//! measures, per glyph, where its ink sits inside the cell — then asserts:
//!   (a) 'H' bottom lands on the baseline (`cell_height - cell_baseline` from the
//!       cell top), within 1px;
//!   (b) 'q' descender extends below 'H' bottom (i.e. below the baseline);
//!   (c) '_' glyph rests on/just below the baseline (a low, baseline-resting
//!       font glyph — distinct from the underline decoration);
//!   (d) sprites (a box-drawing glyph and '❯' when resolvable) share the same
//!       visual baseline as text — no font/sprite vertical mismatch.
//!
//! Skips gracefully (`SKIP:`) when no Metal device is present, matching the
//! R1-R4 GPU-test convention.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
}

/// The ink bounding box of one cell, in cell-local pixel coordinates
/// (`0..cell_w` x `0..cell_h`), where `top`/`bottom` are inclusive and measured
/// from the **top** of the cell. `None` if the cell has no ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ink {
    left: usize,
    right: usize,
    top: usize,
    bottom: usize,
}

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
        }
    }

    /// Ink bbox of cell `(col, row)` relative to `bg`, cell-local coordinates.
    fn cell_ink(&self, col: usize, row: usize, bg: Px, threshold: i32) -> Option<Ink> {
        let mut ink: Option<Ink> = None;
        for dy in 0..self.cell_h {
            for dx in 0..self.cell_w {
                let x = col * self.cell_w + dx;
                let y = row * self.cell_h + dy;
                if x >= self.width || y >= self.height {
                    continue;
                }
                let p = self.px(x, y);
                let d = (p.r as i32 - bg.r as i32).abs()
                    + (p.g as i32 - bg.g as i32).abs()
                    + (p.b as i32 - bg.b as i32).abs();
                if d > threshold {
                    ink = Some(match ink {
                        None => Ink {
                            left: dx,
                            right: dx,
                            top: dy,
                            bottom: dy,
                        },
                        Some(b) => Ink {
                            left: b.left.min(dx),
                            right: b.right.max(dx),
                            top: b.top.min(dy),
                            bottom: b.bottom.max(dy),
                        },
                    });
                }
            }
        }
        ink
    }
}

fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

/// Render `text` starting at (col 0, row 1) and read back the frame + metrics.
fn render_line(size_px: f64, text: &str) -> Option<(Frame, Metrics)> {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping text-baseline test");
            return None;
        }
    };

    let face = Face::load_embedded(size_px).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(face);

    let cols = (text.chars().count() as u16 + 2).max(8);
    let rows = 4u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(b"\x1b[2;1H"); // row 1 (0-indexed), col 0.
    stream.feed(text.as_bytes());
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions::default();
    engine.update_frame(&snapshot, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");

    let (sw, sh) = engine.screen_size();
    assert_eq!(pixels.len(), sw * sh * 4, "readback size");
    Some((
        Frame {
            pixels,
            width: sw,
            height: sh,
            cell_w: cw as usize,
            cell_h: ch as usize,
        },
        metrics,
    ))
}

const BG: Px = Px {
    r: 0x18,
    g: 0x18,
    b: 0x18,
};

/// Core assertion, run at a given pixel size. Returns nothing; skips if no GPU.
fn assert_baseline_at(size_px: f64) {
    // "Hxq_" reveals cap top ('H'), x-height ('x'), descender ('q'), and the
    // underline glyph ('_'). Columns: H=0, x=1, q=2, _=3.
    let Some((frame, metrics)) = render_line(size_px, "Hxq_") else {
        return;
    };
    let row = 1usize;
    let ch = metrics.cell_height as usize;
    // The text baseline, measured from the TOP of the cell.
    let baseline_from_top = ch - metrics.cell_baseline as usize;

    let h = frame
        .cell_ink(0, row, BG, 24)
        .expect("H should have ink coverage");
    let x = frame
        .cell_ink(1, row, BG, 24)
        .expect("x should have ink coverage");
    let q = frame
        .cell_ink(2, row, BG, 24)
        .expect("q should have ink coverage");
    let underscore = frame
        .cell_ink(3, row, BG, 24)
        .expect("_ should have ink coverage");

    eprintln!(
        "size={size_px}px cell={}x{ch} cell_baseline={} baseline_from_top={baseline_from_top} underline_position={}",
        metrics.cell_width, metrics.cell_baseline, metrics.underline_position
    );
    eprintln!("  H ink: {h:?}");
    eprintln!("  x ink: {x:?}");
    eprintln!("  q ink: {q:?}");
    eprintln!("  _ ink: {underscore:?}");

    // (a) 'H' sits ON the baseline: its ink bottom should land within 1px of
    //     the baseline. (Capitals rest exactly on the baseline; a hinting
    //     overshoot of ~1px is the only slack.)
    let h_bottom = h.bottom + 1; // exclusive bottom edge, from cell top
    let delta = h_bottom as i32 - baseline_from_top as i32;
    assert!(
        delta.abs() <= 1,
        "H bottom (={h_bottom}) should land on the baseline (={baseline_from_top}); \
         delta={delta}px. If delta ~= +cell_baseline the glyph is drawn baseline-relative \
         instead of cell-relative (missing +cell_baseline in the CoreText offset_y)."
    );

    // (b) 'q' descends BELOW the baseline: its ink bottom is lower (larger y
    //     from top) than 'H' bottom by a plausible amount.
    assert!(
        q.bottom > h.bottom,
        "q descender ({}) should extend below H bottom ({})",
        q.bottom,
        h.bottom
    );

    // (c) The '_' *glyph* (the ASCII underscore character, a font glyph — not
    //     the underline *decoration*) rests on/just below the text baseline:
    //     its ink sits at or below `baseline_from_top`, low in the cell, below
    //     the x-height. (Distinct from the metric `underline_position`, which
    //     places the drawn underline decoration; the two need not coincide.)
    assert!(
        (underscore.top as i32 - baseline_from_top as i32).abs()
            <= (metrics.cell_height as i32 / 8).max(2),
        "_ glyph top ({}) should rest near the baseline ({baseline_from_top}); \
         it is a low, baseline-resting glyph",
        underscore.top
    );
    assert!(
        underscore.top >= x.top,
        "_ ({underscore:?}) should sit below the x-height (x={x:?})"
    );

    // (d) 'H' cap top sits BELOW the cell top (not clamped to y=0) — a glyph
    //     drawn cell-relative leaves the ascender gap above the cap. A glyph
    //     drawn baseline-relative-too-low pushes the cap down toward mid-cell;
    //     we already caught that in (a). Here we sanity-check the cap doesn't
    //     overflow the top of the cell.
    assert!(
        h.top >= 1 || metrics.cell_baseline == 0,
        "H cap top ({}) should leave an ascender gap above it (>=1px)",
        h.top
    );

    // 'x' x-height sits between the cap top and the baseline.
    assert!(
        x.top > h.top && x.bottom <= h_bottom + 1,
        "x ({x:?}) should sit within the cap-to-baseline band (H={h:?})"
    );
}

#[test]
fn text_baseline_16px() {
    assert_baseline_at(16.0);
}

#[test]
fn text_baseline_32px() {
    assert_baseline_at(32.0);
}

/// Font glyphs and sprites must share the same visual baseline: render a text
/// glyph ('H') and a box-drawing sprite (U+2500 HORIZONTAL LINE) on the same
/// row; the sprite's horizontal rule sits at a metric-derived position, and the
/// text 'H' rests on the cell baseline. Both are placed by the same
/// cell-relative machinery, so neither should be vertically shifted relative to
/// the cell. This is the "font glyph high, sprite cell-exact" mismatch guard.
#[test]
fn text_and_sprite_share_cell_frame() {
    // "H─" : H at col 0 (font glyph), U+2500 at col 1 (sprite).
    let Some((frame, metrics)) = render_line(16.0, "H\u{2500}") else {
        return;
    };
    let row = 1usize;
    let ch = metrics.cell_height as usize;
    let baseline_from_top = ch - metrics.cell_baseline as usize;

    let h = frame.cell_ink(0, row, BG, 24).expect("H ink");
    let line = frame.cell_ink(1, row, BG, 24).expect("box-line ink");

    eprintln!("H={h:?} box-line={line:?} baseline_from_top={baseline_from_top} cell_h={ch}");

    // H rests on the baseline.
    let h_bottom = h.bottom + 1;
    assert!(
        (h_bottom as i32 - baseline_from_top as i32).abs() <= 1,
        "H bottom {h_bottom} not on baseline {baseline_from_top}"
    );
    // The box-drawing horizontal line is a thin rule spanning the full cell
    // width, sitting near the vertical middle of the cell (upstream centers
    // U+2500). It must be well within the cell (not clamped to an edge), which
    // it can only be if the sprite shares the cell coordinate frame with text.
    assert_eq!(line.left, 0, "box line should span from the cell left");
    assert_eq!(
        line.right,
        metrics.cell_width as usize - 1,
        "box line should span to the cell right"
    );
    let line_h = line.bottom - line.top + 1;
    assert!(
        line_h <= (ch / 3).max(2),
        "box line should be a thin rule, got height {line_h}"
    );
    assert!(
        line.top > 0 && line.bottom < ch - 1,
        "box line ({line:?}) should sit inside the cell, not clamped to an edge"
    );
}

/// Cursor row/col sanity vs a starship-style two-line prompt. Feed:
///   line 0: a path segment,
///   line 1: "❯ " then the cursor parks after it.
/// The cursor must land on row 1 (the prompt row), col 2 (just past "❯ ").
/// If the app shows the cursor a row below the ❯, the bug is in the renderer's
/// row mapping, not here — this test pins the snapshot's cursor coordinates.
#[test]
fn cursor_lands_on_prompt_row_and_col() {
    let cols = 20u16;
    let rows = 4u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    // Line 0: a path. CRLF to line 1. Then the prompt marker + trailing space;
    // the cursor is left parked right after them.
    stream.feed("~/code\r\n".as_bytes());
    stream.feed("\u{276F} ".as_bytes()); // "❯ "
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    // The cursor row/col comes straight off the snapshot (no GPU needed).
    let cursor = snapshot_cursor(&snapshot).expect("cursor present in snapshot");
    eprintln!("cursor row={} col={}", cursor.0, cursor.1);

    assert_eq!(cursor.0, 1, "cursor should be on the prompt row (row 1)");
    assert_eq!(
        cursor.1, 2,
        "cursor should be at col 2 (just past the ❯ and its trailing space)"
    );
}

/// Extract (row, col) of the cursor from a captured snapshot.
fn snapshot_cursor(snapshot: &FullSnapshot) -> Option<(usize, usize)> {
    use qwertty_term_renderer::snapshot::RenderSnapshot;
    let c = snapshot.cursor()?;
    Some((c.row, c.col))
}

/// Human-verification artifact: render "Hxq_❯" at 16px, draw a red horizontal
/// rule across the computed text baseline (`cell_height - cell_baseline` from
/// the cell top of the text row), and write it to `target/text-baseline.png`.
/// The cap 'H' and the '❯'/underscore should rest ON this rule; descenders drop
/// below it. Not an assertion (skips without a GPU) — it's a picture to eyeball.
#[test]
fn dump_baseline_artifact() {
    let Some((mut frame, metrics)) = render_line(16.0, "Hxq_\u{276F}") else {
        return;
    };
    let row = 1usize;
    let ch = metrics.cell_height as usize;
    let baseline_y = row * ch + (ch - metrics.cell_baseline as usize);

    // Paint a red rule across the whole frame width at the baseline row.
    if baseline_y < frame.height {
        for x in 0..frame.width {
            let i = (baseline_y * frame.width + x) * 4;
            // BGRA: red = (0, 0, 255).
            frame.pixels[i] = 0;
            frame.pixels[i + 1] = 0;
            frame.pixels[i + 2] = 255;
            frame.pixels[i + 3] = 255;
        }
    }

    if let Some(path) = dump_png(&frame) {
        println!(
            "text-baseline artifact written to {path} (red rule = baseline at y={baseline_y})"
        );
    }
}

/// Write the BGRA frame to `target/text-baseline.png` (minimal PNG encoder,
/// mirrors `cursor_alignment.rs`'s `dump_png`; kept local so each test file is
/// self-contained).
fn dump_png(frame: &Frame) -> Option<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/text-baseline.png"
    );
    let mut rgba = Vec::with_capacity(frame.width * frame.height * 4);
    for chunk in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    let bytes = encode_png(frame.width as u32, frame.height as u32, &rgba)?;
    std::fs::write(path, bytes).ok()?;
    Some(path.to_string())
}

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
