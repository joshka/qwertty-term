//! Pixel-alignment audit: does the cursor rect's on-screen bounding box
//! exactly match a glyph's bounding box, translated by one cell?
//!
//! User-reported: the block cursor looks "subtly misplaced vertically"
//! relative to text. This test renders a full block glyph (U+2588, which
//! fills its entire cell) at `(col+1, row)` next to a block cursor at
//! `(col, row)` over an empty cell, then compares the coverage bounding box
//! of each — translated by one cell width — pixel-for-pixel. If the cursor
//! rect and a glyph-background rect are computed the same way, the two boxes
//! must have identical top/bottom rows and identical width/left-offset
//! within the cell.
//!
//! Skips gracefully (prints `SKIP:`) when no Metal device is present,
//! matching the R1-R4 GPU-test convention (see `first_pixels.rs`).

#![cfg(target_os = "macos")]

use ghostty_font::coretext::Face;
use ghostty_font::grid::Grid;
use ghostty_font::{CodepointResolver, Collection, Metrics};
use ghostty_renderer::engine::{Engine, FrameOptions};
use ghostty_renderer::metal::Metal;
use ghostty_renderer::snapshot::FullSnapshot;
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

/// A read-back BGRA frame with cell-aware sampling.
struct Frame {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    cell_w: usize,
    cell_h: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
}

/// The bounding box of "covered" pixels within one cell, in cell-local pixel
/// coordinates (`0..cell_w`, `0..cell_h`). `None` if nothing in the cell
/// differs from `bg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BBox {
    left: usize,
    right: usize, // inclusive
    top: usize,
    bottom: usize, // inclusive
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

    /// Coverage bounding box of cell `(col, row)` relative to `bg`, in
    /// cell-local pixel coordinates. A pixel counts as "covered" if it
    /// differs from `bg` by more than `threshold` (summed abs channel
    /// delta).
    fn cell_bbox(&self, col: usize, row: usize, bg: Px, threshold: i32) -> Option<BBox> {
        let mut bbox: Option<BBox> = None;
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
                    bbox = Some(match bbox {
                        None => BBox {
                            left: dx,
                            right: dx,
                            top: dy,
                            bottom: dy,
                        },
                        Some(b) => BBox {
                            left: b.left.min(dx),
                            right: b.right.max(dx),
                            top: b.top.min(dy),
                            bottom: b.bottom.max(dy),
                        },
                    });
                }
            }
        }
        bbox
    }
}

fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

#[test]
fn cursor_rect_aligns_with_glyph_cell_box() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping cursor-alignment test");
            return;
        }
    };

    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    // Terminal: cursor at (col=2, row=1); an opaque full-block glyph
    // (U+2588) at (col=3, row=1), immediately to the cursor's right.
    let cols = 10u16;
    let rows = 4u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    // Row 1: two spaces then a full block, matching cols 0,1,2 blank / col 3 block.
    stream.feed(b"\x1b[2;1H"); // row 2 (1-indexed) = row 1 (0-indexed), col 1.
    stream.feed("   \u{2588}".as_bytes()); // cols 0-2 blank, col 3 = full block.
    // Park the cursor at col 2, row 1 (0-indexed): CUP row 2, col 3 (1-indexed).
    stream.feed(b"\x1b[2;3H");
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions::default();
    engine.update_frame(&snapshot, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");

    let (sw, sh) = engine.screen_size();
    assert_eq!(pixels.len(), sw * sh * 4, "readback size");
    let frame = Frame {
        pixels,
        width: sw,
        height: sh,
        cell_w: cw as usize,
        cell_h: ch as usize,
    };

    let bg = Px {
        r: opts.default_bg.r,
        g: opts.default_bg.g,
        b: opts.default_bg.b,
    };

    // The glyph cell (col 3, row 1): the full block should be "covered"
    // across essentially the whole cell.
    let glyph_bbox = frame
        .cell_bbox(3, 1, bg, 24)
        .expect("full block glyph should have coverage");

    // The cursor cell (col 2, row 1): block cursor over an empty cell.
    let cursor_bbox = frame
        .cell_bbox(2, 1, bg, 24)
        .expect("cursor cell should have coverage");

    eprintln!(
        "glyph bbox (col 3): {glyph_bbox:?}  (cell {cw}x{ch})\ncursor bbox (col 2): {cursor_bbox:?}"
    );

    // === THE ASSERTION ===
    // Translated by exactly one cell width, the two boxes' cell-local
    // coordinates must be identical: same top/bottom rows, same left/right
    // columns. (Cell-local coordinates already factor out the one-cell
    // horizontal translation since both are expressed relative to their own
    // cell's origin.)
    assert_eq!(
        cursor_bbox, glyph_bbox,
        "cursor rect bbox does not match glyph cell bbox (translated by one cell); \
         cursor={cursor_bbox:?} glyph={glyph_bbox:?} cell={cw}x{ch}"
    );

    // Extra readability: report against the full cell box too, so a human
    // reading test output can see how close each is to covering [0,cw)x[0,ch).
    eprintln!(
        "full-cell box would be left=0 right={} top=0 bottom={}",
        cw - 1,
        ch - 1
    );

    // === BONUS: dump the frame to a PNG artifact for human comparison. ===
    if let Some(path) = dump_png(&frame) {
        println!("cursor-alignment frame written to {path}");
    }
}

/// Write the BGRA frame to `target/cursor-alignment.png` (hand-rolled minimal
/// PNG encoder, same as `first_pixels.rs`'s `dump_png` — kept local/duplicated
/// rather than shared to keep each test file self-contained).
fn dump_png(frame: &Frame) -> Option<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/cursor-alignment.png"
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
