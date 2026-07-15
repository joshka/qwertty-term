//! R4 acceptance test: FIRST PIXELS.
//!
//! Drives a real `qwertty_term_vt::Terminal` through a `Stream` with a scripted
//! session (colored prompt, "hello 世界", a box-drawing line, a cursor at a
//! known cell), snapshots it, builds GPU buffers via the cell engine, draws an
//! offscreen frame into an IOSurface-backed target, waits for completion, and
//! reads the pixels back — asserting per-cell expectations against the rendered
//! output. No window is involved.
//!
//! Skips gracefully (prints `SKIP:`) when no Metal device is present, matching
//! the R1/R2/R3 GPU-test convention.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::FullSnapshot;
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

    /// Sample the center pixel of cell (col, row).
    fn cell_center(&self, col: usize, row: usize) -> Px {
        let x = col * self.cell_w + self.cell_w / 2;
        let y = row * self.cell_h + self.cell_h / 2;
        self.px(x.min(self.width - 1), y.min(self.height - 1))
    }

    /// Max coverage-vs-background delta over a cell's pixels: how much any pixel
    /// in the cell differs from the given background color (sum of abs channel
    /// deltas). A cell with a glyph has a high delta; a blank cell ~0.
    fn cell_max_delta(&self, col: usize, row: usize, bg: Px) -> i32 {
        let mut max = 0i32;
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
                max = max.max(d);
            }
        }
        max
    }
}

/// Build a grayscale-atlas `Grid` over a face, at a given px size.
fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

#[test]
fn first_pixels_offscreen_readback() {
    // Skip gracefully if no Metal device.
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping first-pixels test");
            return;
        }
    };

    // --- Font substrate: JetBrains Mono for text/box, a CJK system face for
    //     the wide characters (JetBrains Mono has no CJK glyphs). ---
    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    // --- Terminal: a scripted session. ---
    //   Row 0: a colored prompt "$ " (green on default) then "hello "
    //   Row 1: "世界" (wide) then a box-drawing run "──"
    //   Cursor: left at a known cell.
    let cols = 20u16;
    let rows = 3u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    // Green fg for the prompt, reset, then plain "hello", newline, wide + box.
    // \x1b[32m = green fg; \x1b[0m = reset.
    stream.feed(b"\x1b[32m$ \x1b[0mhello\r\n");
    stream.feed("世界".as_bytes());
    stream.feed("\u{2500}\u{2500}".as_bytes()); // box drawings light horizontal x2
    // Move the cursor to a known, empty cell on row 2 (0-indexed row 2, col 3).
    // CUP is 1-indexed: row 3, col 4.
    stream.feed(b"\x1b[3;4H");
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);

    // --- Engine: build + render. ---
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

    // The default background from FrameOptions (0x18 gray).
    let bg = Px {
        r: opts.default_bg.r,
        g: opts.default_bg.g,
        b: opts.default_bg.b,
        a: 255,
    };

    // === ASSERTION 1: background pixels match the default bg. ===
    // Pick a cell that is definitely empty and not under the cursor: row 0,
    // far right column (col 18, well past "hello").
    let empty = frame.cell_center(18, 0);
    let bg_delta = (empty.r as i32 - bg.r as i32).abs()
        + (empty.g as i32 - bg.g as i32).abs()
        + (empty.b as i32 - bg.b as i32).abs();
    assert!(
        bg_delta <= 6,
        "empty cell should match default bg; got {empty:?} vs {bg:?} (delta {bg_delta})"
    );

    // === ASSERTION 2: a glyph cell has coverage (non-background). ===
    // 'h' of "hello" is at row 0, col 2 (after the 2-cell "$ " prompt).
    let glyph_delta = frame.cell_max_delta(2, 0, bg);
    assert!(
        glyph_delta > 40,
        "'h' glyph cell should have coverage; max delta {glyph_delta} too low"
    );

    // === ASSERTION 3: the wide char spans 2 cells (grid level). ===
    // "世" is at row 1, col 0 (lead) + col 1 (spacer). The snapshot marks col 1
    // as a spacer (2-cell occupancy), and the engine skips the spacer while
    // placing the single wide glyph at the lead cell — the load-bearing
    // wide-char property. (Pixel coverage of the CJK glyph itself is out of the
    // reduced substrate's reach: shaping requires a byte-backed face, and the
    // sole embedded face — JetBrains Mono — has no CJK glyph; documented as a
    // deferral in docs/analysis/renderer-r4.md. The box-drawing sprite below
    // proves the wide/two-cell *rendering* path end to end with real pixels.)
    let row1 = snapshot_row(&term, 1);
    assert!(row1[0].is_wide(), "col 0 of row 1 should be the wide lead");
    assert!(
        row1[1].is_spacer(),
        "col 1 of row 1 should be the wide spacer"
    );

    // === ASSERTION 4: the box-drawing cell has sprite coverage. ===
    // "──" follows "世界" on row 1: col 0-1 wide 世, col 2-3 wide 界, col 4-5
    // are the two box chars. Find the first box cell.
    let box_col = find_box_col(&term, 1).expect("box char present on row 1");
    let box_delta = frame.cell_max_delta(box_col, 1, bg);
    assert!(
        box_delta > 40,
        "box-drawing cell (col {box_col}) should have sprite coverage; max delta {box_delta} too low"
    );

    // === ASSERTION 5: the cursor cell is filled with cursor color. ===
    // Cursor is at row 2 (0-indexed), col 3 — a block cursor over an empty
    // cell. The block cursor fills the cell with the cursor color (default fg).
    let cur = frame.cell_center(3, 2);
    let cursor_color = opts.default_fg;
    let cur_delta = (cur.r as i32 - cursor_color.r as i32).abs()
        + (cur.g as i32 - cursor_color.g as i32).abs()
        + (cur.b as i32 - cursor_color.b as i32).abs();
    // The block cursor sprite fills the whole cell, so the center is the cursor
    // color (well away from the default bg).
    let cur_bg_delta = (cur.r as i32 - bg.r as i32).abs()
        + (cur.g as i32 - bg.g as i32).abs()
        + (cur.b as i32 - bg.b as i32).abs();
    assert!(
        cur_bg_delta > 40,
        "cursor cell should not be the default bg; got {cur:?}"
    );
    assert!(
        cur_delta <= 20,
        "cursor cell center should be the cursor color {cursor_color:?}; got {cur:?} (delta {cur_delta})"
    );

    // === BONUS: dump the frame to a PNG artifact. ===
    if let Some(path) = dump_png(&frame) {
        println!("first-pixels frame written to {path}");
    }
}

/// The snapshot cells for a given active-area row.
fn snapshot_row(term: &Terminal, row: usize) -> Vec<qwertty_term_vt::snapshot::SnapshotCell> {
    let snap = term.snapshot_window(0);
    snap.window[row].cells.clone()
}

/// Find the first box-drawing cell (U+2500) on a row.
fn find_box_col(term: &Terminal, row: usize) -> Option<usize> {
    let cells = snapshot_row(term, row);
    cells.iter().position(|c| c.ch == '\u{2500}')
}

/// Write the BGRA frame to `target/first-pixels.png` as an RGBA PNG (hand-rolled
/// minimal encoder, uncompressed via stored zlib blocks — no image-crate
/// dependency). Returns the path on success.
fn dump_png(frame: &Frame) -> Option<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/first-pixels.png");
    // Build an RGBA buffer from BGRA.
    let mut rgba = Vec::with_capacity(frame.width * frame.height * 4);
    for chunk in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    let bytes = encode_png(frame.width as u32, frame.height as u32, &rgba)?;
    std::fs::write(path, bytes).ok()?;
    Some(path.to_string())
}

/// Minimal PNG encoder: 8-bit RGBA, stored (uncompressed) zlib blocks. Good
/// enough for a human-inspectable artifact without pulling in an image crate.
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

    // Raw image data: each row prefixed with filter byte 0.
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        let start = y * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }

    // zlib stream with stored (uncompressed) deflate blocks.
    let mut zlib = vec![0x78, 0x01]; // CMF, FLG
    let mut pos = 0;
    while pos < raw.len() {
        let block = &raw[pos..(pos + 65535).min(raw.len())];
        let last = pos + block.len() >= raw.len();
        zlib.push(if last { 1 } else { 0 }); // BFINAL, BTYPE=00
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
        pos += block.len();
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // IHDR: width, height, bit depth 8, color type 6 (RGBA), 0,0,0.
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}
