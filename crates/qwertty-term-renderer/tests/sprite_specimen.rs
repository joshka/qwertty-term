//! Sprite specimen sheet: one row per procedural-glyph family, rendered
//! offscreen, with an ink-per-row assertion.
//!
//! Drives a real `qwertty_term_vt::Terminal` through a `Stream`, feeding a specimen
//! sheet — rows 0–2: box drawing (light, double, rounded); row 3: block
//! elements/shades and braille; row 4: powerline separators; row 5: legacy
//! computing symbols; row 6: diagonals and dashes. It snapshots the terminal,
//! builds cell buffers via the cell engine, draws an offscreen frame over the
//! platform-free [`Software`] backend, and reads the pixels back. The assertion
//! is deliberately coarse: every specimen row must contain ink (coverage above
//! background) in at least one cell — visual quality is judged from the PNG
//! dumped to `target/sprite-specimen.png`. No window is involved.
//!
//! Runs over the Software backend (ADR 003), so it **never skips** and is **not
//! OS-gated** — sprites are pure Rust (`qwertty-term-sprite`), so this gives real
//! Linux sprite pixel coverage (#42).

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
fn sprite_specimen_offscreen_readback() {
    let backend = Software::new();

    // --- Font substrate: embedded JetBrains Mono supplies the metrics (cell
    //     size); the specimen glyphs themselves are drawn by the procedural
    //     sprite renderer, not rasterized from the font. ---
    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    // --- Terminal: the specimen sheet, one row per procedural-glyph family.
    //     Rows 0-2: box drawing (light, double, rounded corners); row 3: block
    //     elements/shades and braille; row 4: powerline separators; row 5:
    //     legacy computing symbols; row 6: diagonals and dashes. ---
    let cols = 30u16;
    let rows = 8u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed("\u{250C}\u{2500}\u{252C}\u{2500}\u{2510} \u{2554}\u{2550}\u{2566}\u{2550}\u{2557} \u{256D}\u{2500}\u{256E}\r\n".as_bytes());
    stream.feed(
        "\u{2502} \u{2502} \u{2502} \u{2551} \u{2551} \u{2551} \u{2502} \u{2502}\r\n".as_bytes(),
    );
    stream.feed("\u{2514}\u{2500}\u{2534}\u{2500}\u{2518} \u{255A}\u{2550}\u{2569}\u{2550}\u{255D} \u{2570}\u{2500}\u{256F}\r\n".as_bytes());
    stream.feed("\u{2588}\u{2593}\u{2592}\u{2591} \u{2580}\u{2584}\u{258C}\u{2590} \u{28FF}\u{28F7}\u{2847}\u{2801}\r\n".as_bytes());
    stream.feed("\u{E0B0}\u{E0B1} \u{E0B2}\u{E0B3} \u{E0B4}\u{E0B6} sep\r\n".as_bytes());
    stream.feed(
        "\u{1FB00}\u{1FB01}\u{1FB3B}\u{1FB44} \u{1FB95}\u{1FB98}\u{1FB99} legacy\r\n".as_bytes(),
    );
    stream.feed("\u{2571}\u{2572}\u{2573} \u{2504}\u{2508}\u{254C} dash/diag".as_bytes());
    // The cursor stays where the last feed left it (row 6, past the scanned
    // columns). Don't park it inside the specimen area: the block cursor is
    // drawn by default and would satisfy a row's ink assertion on its own,
    // masking a broken sprite family.
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

    // Generic specimen assertions: every specimen row (0..=6) must contain ink
    // (coverage above background) in at least one cell — each sprite family
    // rendered SOMETHING; visual quality is judged from the dumped PNG.
    if let Some(path) = dump_png(&frame) {
        eprintln!("specimen dumped early: {path}");
    }
    eprintln!("screen dump:\n{}", term.plain_string());
    let bg = frame.px(15, 0); // top-right region is blank background
    let mut missing = Vec::new();
    for row in 0..=6usize {
        let ink = (0..14).any(|col| frame.cell_max_delta(col, row, bg) > 40);
        if !ink {
            missing.push(row);
        }
    }
    assert!(missing.is_empty(), "specimen rows with no ink: {missing:?}");

    if let Some(path) = dump_png(&frame) {
        println!("first-pixels frame written to {path}");
    }
}

/// Write the BGRA frame to `target/sprite-specimen.png` as an RGBA PNG (hand-rolled
/// minimal encoder, uncompressed via stored zlib blocks — no image-crate
/// dependency). Returns the path on success.
fn dump_png(frame: &Frame) -> Option<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/sprite-specimen.png"
    );
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
