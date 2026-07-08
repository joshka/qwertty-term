//! Debug frame-dump for the windowed present path (env-gated).
//!
//! When `QWERTTY_TERM_APP_DUMP_FRAME` is set to a path prefix, the window host reads
//! back the presented IOSurface after every Nth present and writes it to
//! `<prefix>-<seq>.png`. This is the decisive experiment for the "blank window"
//! class of bugs: it captures *exactly the bytes attached to the CoreAnimation
//! layer*, so inspecting the PNGs discriminates a presentation-geometry bug
//! (PNG contains glyphs, but the window shows none → contentsScale / layer
//! mismatch) from a pump/draw bug (PNG itself is blank → nothing was drawn).
//!
//! The encoder is a tiny, dependency-free PNG writer using stored (uncompressed)
//! DEFLATE blocks, so the app crate gains no new dependency. Input is BGRA
//! (the IOSurface / [`qwertty_term_renderer::metal::Target::read_pixels`] layout);
//! it is swizzled to RGBA and emitted as 8-bit truecolor+alpha.

use std::io::Write;

/// Per-tab dump state: the configured path prefix and a running sequence.
pub struct FrameDump {
    prefix: String,
    every: u64,
    tick: u64,
    seq: u64,
}

impl FrameDump {
    /// Build the dump config from the environment, or `None` if disabled.
    ///
    /// `QWERTTY_TERM_APP_DUMP_FRAME` — path prefix (enables dumping).
    /// `QWERTTY_TERM_APP_DUMP_EVERY` — dump every Nth present (default 30, ~0.5s at
    /// 60Hz); clamped to at least 1.
    pub fn from_env() -> Option<Self> {
        let prefix = std::env::var("QWERTTY_TERM_APP_DUMP_FRAME").ok()?;
        if prefix.is_empty() {
            return None;
        }
        let every = std::env::var("QWERTTY_TERM_APP_DUMP_EVERY")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30)
            .max(1);
        Some(Self {
            prefix,
            every,
            tick: 0,
            seq: 0,
        })
    }

    /// Whether this present tick should be dumped (advances the internal tick).
    pub fn should_dump(&mut self) -> bool {
        let due = self.tick.is_multiple_of(self.every);
        self.tick += 1;
        due
    }

    /// Write a BGRA frame to the next `<prefix>-<seq>.png`. Errors are logged to
    /// stderr but never propagated (dumping is a debug aid, not a gate).
    pub fn write(&mut self, bgra: &[u8], width: usize, height: usize) {
        let path = format!("{}-{:04}.png", self.prefix, self.seq);
        self.seq += 1;
        match encode_png_bgra(bgra, width, height) {
            Ok(png) => match std::fs::File::create(&path).and_then(|mut f| f.write_all(&png)) {
                Ok(()) => eprintln!("QWERTTY_TERM_APP_DUMP_FRAME: wrote {path} ({width}x{height})"),
                Err(e) => eprintln!("QWERTTY_TERM_APP_DUMP_FRAME: write {path} failed: {e}"),
            },
            Err(e) => eprintln!("QWERTTY_TERM_APP_DUMP_FRAME: encode failed: {e}"),
        }
    }
}

/// Coverage metric shared by the dump path and the pixel-assertion smoke: the
/// maximum per-pixel L1 distance from the terminal's default background. A frame
/// with real glyph coverage has some pixel far from the background; a blank
/// clear leaves every pixel at (or very near) the background.
///
/// `bg` is the expected background as `(r, g, b)`. Input is BGRA.
pub fn max_bg_delta(bgra: &[u8], bg: (u8, u8, u8)) -> i32 {
    let mut max_delta = 0i32;
    for px in bgra.chunks_exact(4) {
        let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
        let d = (r - bg.0 as i32).abs() + (g - bg.1 as i32).abs() + (b - bg.2 as i32).abs();
        max_delta = max_delta.max(d);
    }
    max_delta
}

/// Mean Rec. 601 luma of a BGRA pixel buffer in `[0.0, 255.0]`
/// (`0.299R + 0.587G + 0.114B`). Used by the splits smoke to measure a pane's
/// overall brightness: an unfocused-dimmed pane's mean luma sits measurably
/// below the same pane when focused.
pub fn mean_luma(bgra: &[u8]) -> f64 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for px in bgra.chunks_exact(4) {
        let (b, g, r) = (px[0] as f64, px[1] as f64, px[2] as f64);
        sum += 0.299 * r + 0.587 * g + 0.114 * b;
        n += 1;
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// Encode a BGRA pixel buffer as an 8-bit RGBA truecolor PNG with no
/// compression (stored DEFLATE blocks). Dependency-free.
fn encode_png_bgra(bgra: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    if bgra.len() != width * height * 4 {
        return Err(format!(
            "buffer {} != {}x{}x4 ({})",
            bgra.len(),
            width,
            height,
            width * height * 4
        ));
    }

    // Build the raw image data: each scanline is prefixed with filter byte 0
    // (None), pixels swizzled BGRA -> RGBA.
    let mut raw = Vec::with_capacity(height * (1 + width * 4));
    for row in 0..height {
        raw.push(0u8); // filter: None
        let line = &bgra[row * width * 4..(row + 1) * width * 4];
        for px in line.chunks_exact(4) {
            raw.push(px[2]); // R
            raw.push(px[1]); // G
            raw.push(px[0]); // B
            raw.push(px[3]); // A
        }
    }

    let mut out = Vec::new();
    // PNG signature.
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);

    // IHDR.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: truecolor + alpha
    ihdr.push(0); // compression: deflate
    ihdr.push(0); // filter: adaptive
    ihdr.push(0); // interlace: none
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT: zlib stream wrapping stored (uncompressed) DEFLATE blocks.
    let idat = zlib_stored(&raw);
    write_chunk(&mut out, b"IDAT", &idat);

    // IEND.
    write_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

/// Write one PNG chunk (length, type, data, CRC32).
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// Wrap `data` in a zlib stream using stored DEFLATE blocks (no compression).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 8);
    // zlib header: CMF=0x78 (deflate, 32K window), FLG=0x01 (no dict, check).
    out.push(0x78);
    out.push(0x01);

    // Stored blocks: max 65535 bytes each.
    let mut chunks = data.chunks(65535).peekable();
    if data.is_empty() {
        // One empty final block.
        out.push(1); // BFINAL=1, BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        while let Some(chunk) = chunks.next() {
            let last = chunks.peek().is_none();
            out.push(if last { 1 } else { 0 }); // BFINAL, BTYPE=00 (stored)
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    // Adler-32 of the uncompressed data.
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 checksum (zlib trailer).
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Minimal CRC-32 (IEEE) for PNG chunks.
struct Crc32 {
    crc: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { crc: 0xFFFF_FFFF }
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let mut c = (self.crc ^ byte as u32) & 0xFF;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            self.crc = (self.crc >> 8) ^ c;
        }
    }

    fn finalize(self) -> u32 {
        self.crc ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_bg_delta_zero_for_uniform_bg() {
        let bg = (0x18, 0x18, 0x18);
        // 4 pixels all at bg (BGRA).
        let px = [0x18, 0x18, 0x18, 0xFF].repeat(4);
        assert_eq!(max_bg_delta(&px, bg), 0);
    }

    #[test]
    fn max_bg_delta_detects_bright_pixel() {
        let bg = (0x18, 0x18, 0x18);
        let mut px = [0x18, 0x18, 0x18, 0xFF].repeat(4);
        // Make the 3rd pixel white (BGRA).
        px[8] = 0xFF;
        px[9] = 0xFF;
        px[10] = 0xFF;
        // delta = |0xFF-0x18|*3 = 231*3 = 693
        assert_eq!(max_bg_delta(&px, bg), (0xFF - 0x18) * 3);
    }

    #[test]
    fn png_roundtrips_through_image_decoder() {
        // 2x2 BGRA: red, green, blue, white.
        let bgra = [
            0x00, 0x00, 0xFF, 0xFF, // red   (BGRA)
            0x00, 0xFF, 0x00, 0xFF, // green
            0xFF, 0x00, 0x00, 0xFF, // blue
            0xFF, 0xFF, 0xFF, 0xFF, // white
        ];
        let png = encode_png_bgra(&bgra, 2, 2).expect("encode");
        // Signature + at least IHDR/IDAT/IEND present.
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
        );
        assert!(png.windows(4).any(|w| w == b"IHDR"));
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.windows(4).any(|w| w == b"IEND"));

        // Decode with the `image` crate (a dev-only reader) to prove the bytes
        // are a valid PNG and the pixels swizzled correctly.
        let img = image::load_from_memory(&png).expect("valid png").to_rgba8();
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [0xFF, 0x00, 0x00, 0xFF]); // red
        assert_eq!(img.get_pixel(1, 0).0, [0x00, 0xFF, 0x00, 0xFF]); // green
        assert_eq!(img.get_pixel(0, 1).0, [0x00, 0x00, 0xFF, 0xFF]); // blue
        assert_eq!(img.get_pixel(1, 1).0, [0xFF, 0xFF, 0xFF, 0xFF]); // white
    }
}
