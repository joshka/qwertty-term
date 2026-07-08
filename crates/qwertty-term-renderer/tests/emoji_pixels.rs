//! Color-atlas acceptance test: EMOJI PIXELS.
//!
//! Drives a real `qwertty_term_vt::Terminal` through a `Stream` with the line
//! "hi 😀 🦀 🥋 ok" (the last two are the field-reported cases that rendered as
//! "L"-shaped replacement glyphs before the color atlas was wired), snapshots
//! it, builds GPU buffers via the cell engine (which
//! rasterizes the emoji through a discovered Apple-Color-Emoji fallback face
//! into the **color (BGRA) atlas** and tags the instance `CellText.atlas =
//! Color`), draws an offscreen frame, and reads the pixels back — asserting the
//! emoji cell(s) contain SATURATED COLOR pixels (a chroma signal) while the
//! ASCII cells are monochrome. Dumps a PNG artifact for human inspection.
//!
//! This is the end-to-end proof that the color atlas is wired through the
//! renderer: BGRA texture upload (`sync_atlas` → color atlas), the two-texture
//! bind in the cell_text draw, the `CellText.atlas` selector, and the emoji
//! cell-fit constraint sizing.
//!
//! Skips gracefully (prints `SKIP:`) when no Metal device or no color emoji
//! face is present, matching the GPU-test convention.

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

    /// Max chroma over a cell's pixels: `max(|r-g|, |g-b|, |r-b|)`. A grayscale
    /// (monochrome) glyph has chroma ~0 on every pixel (r==g==b); a color emoji
    /// has some saturated pixels with high chroma. This is the load-bearing
    /// "is this cell colored?" signal.
    fn cell_max_chroma(&self, col: usize, row: usize) -> i32 {
        let mut max = 0i32;
        for dy in 0..self.cell_h {
            for dx in 0..self.cell_w {
                let x = col * self.cell_w + dx;
                let y = row * self.cell_h + dy;
                if x >= self.width || y >= self.height {
                    continue;
                }
                let p = self.px(x, y);
                let rg = (p.r as i32 - p.g as i32).abs();
                let gb = (p.g as i32 - p.b as i32).abs();
                let rb = (p.r as i32 - p.b as i32).abs();
                max = max.max(rg).max(gb).max(rb);
            }
        }
        max
    }

    /// Max coverage-vs-background delta over a cell's pixels (sum of abs channel
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

/// Build a `Grid` over the embedded primary face; the resolver discovers color
/// emoji / CJK fallbacks via CoreText at render time.
fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

/// Column of the first cell whose codepoint is the emoji, at a row.
fn emoji_col(term: &Terminal, row: usize, emoji: char) -> Option<usize> {
    let snap = term.snapshot_window(0);
    snap.window[row].cells.iter().position(|c| c.ch == emoji)
}

#[test]
fn emoji_pixels_offscreen_readback() {
    // Skip gracefully if no Metal device.
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping emoji-pixels test");
            return;
        }
    };

    // Primary face: embedded JetBrains Mono (ASCII). The emoji resolves to a
    // discovered Apple Color Emoji fallback face at render time.
    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    // The emoji we render + assert on. 😀 is a general smoke; 🦀 (U+1F980) and
    // 🥋 (U+1F94B) are the field-reported cases that showed as "L"-shaped
    // replacement glyphs before the color atlas was wired (a BGRA glyph drawn
    // through the grayscale sampler reads garbage). Each is width-2.
    //
    // Chroma note: 😀 (yellow) and 🦀 (orange) carry strong saturated color, so
    // they get the strict readback chroma check. 🥋 (a mostly-white gi with a
    // small belt) genuinely rasterizes near-grayscale at cell size — its
    // Apple-Color-Emoji bitmap has ~no saturated pixels at ~20px — so a chroma
    // check would be a false negative. For 🥋 we instead assert the load-bearing
    // "not an L" property: it routes through the *color* atlas with coverage
    // (verified at the grid level in the preflight + a coverage check below).
    const EMOJIS: [char; 3] = ['😀', '🦀', '🥋'];
    const CHROMA_EMOJIS: [char; 2] = ['😀', '🦀'];

    // Preflight: does the resolver actually reach a COLOR face for the emoji on
    // this machine? If not (headless CI without emoji fonts), skip cleanly
    // rather than fail.
    for e in EMOJIS {
        match grid.render_codepoint(e as u32) {
            Ok(Some(g)) if g.atlas == qwertty_term_font::AtlasKind::Color && g.width > 0 => {}
            other => {
                eprintln!(
                    "SKIP: emoji {e:?} did not resolve to a non-empty color glyph \
                     ({other:?}); no color emoji face available"
                );
                return;
            }
        }
    }

    // Terminal: "hi 😀 🦀 🥋 ok". Each emoji is width-2 (wide lead + spacer).
    let cols = 30u16;
    let rows = 1u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed("hi 😀 🦀 🥋 ok".as_bytes());
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);

    // Engine: build + render.
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions {
        // No cursor for this test (keep the line pixels clean of a block fill).
        cursor_blink_visible: false,
        ..FrameOptions::default()
    };
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
        a: 255,
    };

    let snap = term.snapshot_window(0);
    let row0 = &snap.window[0].cells;

    // === ASSERTION 1: every emoji is a 2-cell (wide lead + spacer) span that
    // renders with coverage (not blank, not an "L" replacement box). The glyph
    // is placed from the lead cell but the width-2 constraint lets it fill both
    // cells, so check the whole span. ===
    for e in EMOJIS {
        let ecol = emoji_col(&term, 0, e).unwrap_or_else(|| panic!("emoji {e:?} present"));
        assert!(row0[ecol].is_wide(), "emoji {e:?} lead cell should be wide");
        assert!(
            row0[ecol + 1].is_spacer(),
            "emoji {e:?} should span 2 cells (spacer tail)"
        );

        let ecov = frame
            .cell_max_delta(ecol, 0, bg)
            .max(frame.cell_max_delta(ecol + 1, 0, bg));
        assert!(
            ecov > 40,
            "emoji {e:?} span should have glyph coverage; max delta {ecov}"
        );
    }

    // === ASSERTION 1b: the chromatic emoji contain SATURATED COLOR pixels
    // (chroma signal), distinguishing a real color glyph from a grayscale
    // "L"-shaped replacement drawn through the wrong sampler. ===
    for e in CHROMA_EMOJIS {
        let ecol = emoji_col(&term, 0, e).unwrap();
        let chroma_lead = frame.cell_max_chroma(ecol, 0);
        let chroma_tail = frame.cell_max_chroma(ecol + 1, 0);
        let chroma = chroma_lead.max(chroma_tail);
        assert!(
            chroma > 30,
            "emoji {e:?} span (cols {ecol}..={}) should contain saturated color \
             pixels; max chroma {chroma} (lead {chroma_lead}, tail {chroma_tail}) too low",
            ecol + 1
        );
    }

    // === ASSERTION 2: the ASCII cells are MONOCHROME. ===
    // 'h','i','o','k' are grayscale text: r==g==b on every pixel, so chroma ~0.
    // Locate them dynamically ('o' and 'k' shift with the emoji columns).
    let ascii_cols: Vec<(char, usize)> = ['h', 'i', 'o', 'k']
        .into_iter()
        .filter_map(|c| {
            row0.iter()
                .position(|cell| cell.ch == c)
                .map(|col| (c, col))
        })
        .collect();
    for (ch, col) in ascii_cols {
        let label = ch;
        // Only assert on cells that actually carry a glyph.
        let cov = frame.cell_max_delta(col, 0, bg);
        if cov <= 40 {
            continue; // face didn't render this cell (shouldn't happen for ASCII)
        }
        let c = frame.cell_max_chroma(col, 0);
        assert!(
            c <= 12,
            "ASCII '{label}' cell (col {col}) should be monochrome; chroma {c} too high"
        );
    }

    // === BONUS: dump the frame to a PNG artifact. ===
    if let Some(path) = dump_png(&frame) {
        println!("emoji-pixels frame written to {path}");
    }
}

/// Write the BGRA frame to `target/emoji-pixels.png` as an RGBA PNG (hand-rolled
/// minimal encoder — no image-crate dependency). Returns the path on success.
fn dump_png(frame: &Frame) -> Option<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/emoji-pixels.png");
    let mut rgba = Vec::with_capacity(frame.width * frame.height * 4);
    for chunk in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    let bytes = encode_png(frame.width as u32, frame.height as u32, &rgba)?;
    std::fs::write(path, bytes).ok()?;
    Some(path.to_string())
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
