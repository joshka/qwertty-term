//! Family-styles specimen: render a line of text in the configured family's
//! **regular / bold / italic** faces and dump a human-inspectable PNG to
//! `target/family-styles.png`.
//!
//! This is the visual evidence for the named-family styled-face path
//! ([`Collection::new_with_family_styles`]): with `font-family = "FiraCode
//! Nerd Font Mono"`, the bold row is FiraCode's *own* Bold member (not the
//! embedded JetBrains Mono Bold), and the italic row is a synthetic-skewed
//! FiraCode (FiraCode has no italic member). No window/GPU is involved — glyphs
//! are rasterized straight from the CoreText faces and composited onto an RGBA
//! canvas.
//!
//! Skips gracefully (prints `SKIP:`) when FiraCode Nerd Font Mono is not
//! installed, matching the discovery-test convention.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::{Collection, Metrics, Style};

const FAMILY: &str = "FiraCode Nerd Font Mono";
const SIZE_PX: f64 = 32.0; // 2x the configured 16pt for a crisper specimen.
const SPECIMEN: &str = "The quick brown fox: 0O1lI +=> <->";

/// One rendered row: a label and the face to render the sample text with.
struct Row {
    label: &'static str,
    style: Style,
}

#[test]
fn family_styles_specimen_png() {
    // Discovery gate: skip if the configured family isn't installed.
    if qwertty_term_font::discovery::discover_family_style(FAMILY, false, false, SIZE_PX).is_none()
    {
        eprintln!("SKIP: {FAMILY} not installed; family-styles specimen PNG skipped");
        return;
    }

    let primary = Face::load_by_name(FAMILY, SIZE_PX).expect("load FiraCode primary");
    assert!(
        primary.family_name().to_lowercase().contains("fira"),
        "primary should resolve to FiraCode, got {:?}",
        primary.family_name()
    );

    let metrics = Metrics::calc(primary.face_metrics());
    let cell_w = metrics.cell_width as usize;
    let cell_h = metrics.cell_height as usize;
    let baseline = metrics.cell_baseline as usize;

    let collection =
        Collection::new_with_family_styles(primary, FAMILY, SIZE_PX).expect("build family chain");

    let rows = [
        Row {
            label: "regular",
            style: Style::Regular,
        },
        Row {
            label: "bold   ",
            style: Style::Bold,
        },
        Row {
            label: "italic ",
            style: Style::Italic,
        },
    ];

    // Canvas layout: a left gutter for the row label (rendered in the regular
    // face) plus the specimen text, one row per style with a blank separator.
    let gutter_cols = 9usize; // "regular: " width.
    let text_cols = SPECIMEN.chars().count();
    let cols = gutter_cols + text_cols + 1;
    let row_pitch = cell_h + cell_h / 3; // a little vertical breathing room.
    let width = cols * cell_w;
    let height = rows.len() * row_pitch + row_pitch / 2;

    // White background, opaque.
    let mut rgba = vec![0xFFu8; width * height * 4];

    for (i, row) in rows.iter().enumerate() {
        let y0 = row_pitch / 2 + i * row_pitch;
        // Label in the regular face.
        let regular = collection.face_for_style(Style::Regular).unwrap();
        draw_text(
            &mut rgba,
            width,
            height,
            regular,
            &format!("{}:", row.label),
            0,
            y0,
            cell_w,
            baseline,
        );
        // Sample text in the row's style.
        let face = collection
            .face_for_style(row.style)
            .expect("style populated");
        draw_text(
            &mut rgba,
            width,
            height,
            face,
            SPECIMEN,
            gutter_cols,
            y0,
            cell_w,
            baseline,
        );
    }

    let bytes = encode_png(width as u32, height as u32, &rgba);
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/family-styles.png"
    );
    std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")).ok();
    std::fs::write(path, bytes).expect("write specimen PNG");
    eprintln!("family-styles specimen PNG written to {path}");
}

/// Composite a run of glyphs from `face`, one per cell, onto the RGBA canvas as
/// black-on-white ink (Alpha8 coverage → darkness). `col0`/`y_top` are the
/// starting cell column and the pixel row of the cell's top edge.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    face: &Face,
    text: &str,
    col0: usize,
    y_top: usize,
    cell_w: usize,
    baseline: usize,
) {
    for (i, ch) in text.chars().enumerate() {
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let Ok(bmp) = face.rasterize(gid) else {
            continue;
        };
        if bmp.width == 0 || bmp.height == 0 {
            continue;
        }
        let bw = bmp.width as usize;
        let bh = bmp.height as usize;
        // Cell origin in pixels.
        let cell_x = (col0 + i) * cell_w;
        // Ink placement: bearing_x from the cell's left; bearing_y is measured
        // from the cell bottom to the ink top, so the ink top row sits at
        // (baseline - bearing_y) below the cell top... but our metrics baseline
        // is measured from the cell bottom too, so top = y_top + (cell_h -
        // baseline) - bearing_y where cell_h ~= baseline + ascent. Use the
        // ascii box: ink top = y_top + ascent - bearing_y, ascent = baseline.
        let ink_left = cell_x as i32 + bmp.bearing_x;
        let ink_top = y_top as i32 + baseline as i32 - bmp.bearing_y;
        for by in 0..bh {
            let py = ink_top + by as i32;
            if py < 0 || py as usize >= height {
                continue;
            }
            for bx in 0..bw {
                let px = ink_left + bx as i32;
                if px < 0 || px as usize >= width {
                    continue;
                }
                // Alpha8: coverage 0..255. Blend black ink over the current
                // (white) background: out = bg * (1 - a).
                let a = bmp.data[by * bw + bx] as u32;
                if a == 0 {
                    continue;
                }
                let idx = (py as usize * width + px as usize) * 4;
                for c in 0..3 {
                    let bg = rgba[idx + c] as u32;
                    rgba[idx + c] = ((bg * (255 - a)) / 255) as u8;
                }
                rgba[idx + 3] = 0xFF;
            }
        }
    }
}

/// Minimal PNG encoder: 8-bit RGBA, stored (uncompressed) zlib blocks. Adapted
/// from the sprite-specimen encoder so this test needs no image-crate dep.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
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
    out
}
