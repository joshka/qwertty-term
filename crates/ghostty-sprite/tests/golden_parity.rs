//! Golden-parity harness: compare this crate's rasterization against upstream
//! Ghostty's own sprite golden PNGs.
//!
//! # What upstream provides
//!
//! Upstream (`src/font/sprite/Face.zig`, `test "sprite face render all
//! sprites"`) renders every sprite codepoint into a set of **16x16 glyph
//! atlases** — one PNG per Unicode range of 256 codepoints — at four cell
//! metrics, and diffs them against checked-in reference PNGs under
//! `src/font/sprite/testdata/`. Those reference PNGs are what we compare
//! against here (copied verbatim into `tests/testdata/`).
//!
//! Each fixture is named `U+{min}...U+{max}-{W}x{H}+{T}.png` where the atlas
//! holds codepoints `min..=max` (a 0x100-aligned block) laid out in a 16x16
//! grid. Each grid cell is a padded box of `stride_x = W + 2*(W/4)` by
//! `stride_y = H + 2*(H/4)` pixels; the glyph itself occupies the inner `W x H`
//! at offset `(W/4, H/4)`. The PNGs are 8-bit alpha (grayscale) coverage — the
//! exact bytes this crate's `Glyph.alpha` produces.
//!
//! # The four upstream metrics
//!
//! Upstream calls `testDrawRanges(width, ascent, descent, thickness)` with:
//!   (18, 30, 6, 4)  (12, 20, 4, 3)  (11, 19, 2, 2)  (9, 15, 2, 1)
//! giving cell `W x H = width x (ascent+descent)` and box/underline thickness
//! `T`. Crucially the box line thickness is passed *explicitly* (upstream's
//! `Metrics.calc` sets `box_thickness = underline_thickness = ceil(thickness)`),
//! so we must construct [`Metrics`] with that exact `box_thickness` rather than
//! using [`Metrics::simple`], whose heuristic thickness would not match.
//!
//! # Comparison metric
//!
//! We reconstruct each atlas from this crate's glyphs and compare it to the
//! reference pixel-for-pixel. Two thresholds, applied per range (see
//! [`FAMILIES`]):
//!   * `max_pixel_delta` — the largest allowed absolute difference in any single
//!     coverage byte. `0` means exact (integer-path glyphs: box, blocks,
//!     sextants, octants, braille dots that are axis-aligned rectangles).
//!   * `max_diff_fraction` — the largest allowed fraction of pixels that differ
//!     at all. Absorbs sub-pixel antialiasing differences between tiny-skia and
//!     upstream's z2d on curves and diagonals.

use ghostty_sprite::{Metrics, render};

/// One upstream metrics configuration.
struct SizeCfg {
    width: u32,
    ascent: u32,
    descent: u32,
    thickness: u32,
}

impl SizeCfg {
    fn height(&self) -> u32 {
        self.ascent + self.descent
    }

    /// Build the [`Metrics`] matching upstream's `Metrics.calc` for this config.
    ///
    /// Only the fields that affect sprite geometry need to be exact:
    /// `cell_width`, `cell_height`, and `box_thickness` (= underline thickness
    /// upstream). The decoration/cursor fields are irrelevant to the atlas
    /// ranges compared here (those ranges contain no cursor/decoration
    /// pseudo-glyphs), but we fill them consistently anyway.
    fn metrics(&self) -> Metrics {
        let w = self.width;
        let h = self.height();
        let t = self.thickness;
        Metrics {
            cell_width: w,
            cell_height: h,
            cell_baseline: self.descent,
            underline_position: h.saturating_sub(t * 2),
            underline_thickness: t,
            strikethrough_position: h / 2,
            strikethrough_thickness: t,
            overline_position: 0,
            overline_thickness: t,
            box_thickness: t,
            cursor_thickness: t.max(1),
            cursor_height: h,
        }
    }
}

const SIZES: &[SizeCfg] = &[
    SizeCfg {
        width: 18,
        ascent: 30,
        descent: 6,
        thickness: 4,
    },
    SizeCfg {
        width: 12,
        ascent: 20,
        descent: 4,
        thickness: 3,
    },
    SizeCfg {
        width: 11,
        ascent: 19,
        descent: 2,
        thickness: 2,
    },
    SizeCfg {
        width: 9,
        ascent: 15,
        descent: 2,
        thickness: 1,
    },
];

/// A comparison "family": one 0x100-aligned Unicode block, with the tolerance
/// it is held to and a human label.
struct Family {
    /// First codepoint of the 256-wide block (its fixture base).
    base: u32,
    /// Human name used in the report.
    label: &'static str,
    /// Largest allowed absolute per-pixel coverage delta (0 = exact).
    max_pixel_delta: u8,
    /// Largest allowed fraction of differing pixels (0.0 = none may differ).
    max_diff_fraction: f64,
}

/// Every range upstream ships a golden for. Tolerances are set per the task's
/// priority: integer-path families exact; AA-heavy families a small perceptual
/// budget. See `docs/analysis/sprite.md` for the justification of each.
const FAMILIES: &[Family] = &[
    // Box drawing + block elements + geometric shapes (U+2500..U+25FF).
    // Every straight line, junction, corner, tee, cross, and block element is
    // pixel-exact; only the rounded corners (U+256D..2570), diagonals
    // (U+2571..2573) and triangles (U+25E2.., U+25F8..) antialias. Measured
    // worst case is 0.48% at 9x17; 1% leaves regression headroom.
    Family {
        base: 0x2500,
        label: "box/block/geometric (U+2500)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.01,
    },
    // Braille (U+2800..U+28FF): dots drawn as integer rectangles. Pixel-exact
    // at every size, so held to zero.
    Family {
        base: 0x2800,
        label: "braille (U+2800)",
        max_pixel_delta: 0,
        max_diff_fraction: 0.0,
    },
    // Powerline (U+E000..U+E0FF): triangles, arcs, flames — heavy diagonals and
    // curves. Worst case 0.62% at 9x17; 2% headroom.
    Family {
        base: 0xE000,
        label: "powerline (U+E000)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.02,
    },
    // Git-branch symbols (U+F500..U+F5FF, U+F600..U+F6FF): curved strokes.
    // Worst case 1.27% (F500 @ 9x17); 2% headroom.
    Family {
        base: 0xF500,
        label: "branch (U+F500)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.02,
    },
    Family {
        base: 0xF600,
        label: "branch (U+F600)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.02,
    },
    // Legacy computing (U+1FB00..U+1FBFF): sextants + shades are pixel-exact,
    // but this is the most diagonal/curve-dense range (diagonal-hatch fills
    // U+1FB98/99, rounded-diagonal boxes U+1FBA0.., curves U+1FBD0..). Upstream
    // itself flags the hatch fills as imperfectly aligned. Worst case is 3.03%
    // at the smallest 9x17 cell, where AA on near-vertical diagonals diverges
    // most; 4% budget covers it with headroom. Larger sizes stay under 2.3%.
    Family {
        base: 0x1FB00,
        label: "legacy computing (U+1FB00)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.04,
    },
    // Symbols for Legacy Computing Supplement (U+1CC00..U+1CCFF).
    Family {
        base: 0x1CC00,
        label: "legacy supplement (U+1CC00)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.01,
    },
    // Octants (U+1CD00..U+1CDFF): pure integer 2x4 block partitions — exact.
    Family {
        base: 0x1CD00,
        label: "octants (U+1CD00)",
        max_pixel_delta: 0,
        max_diff_fraction: 0.0,
    },
    // Legacy supplement continued (U+1CE00..U+1CEFF): separated block/shade
    // fills. Worst case 0.24% at 9x17; 1% headroom.
    Family {
        base: 0x1CE00,
        label: "legacy supplement (U+1CE00)",
        max_pixel_delta: 255,
        max_diff_fraction: 0.01,
    },
];

/// The alpha (grayscale) coverage of one reconstructed 16x16 atlas.
struct Atlas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

/// Reconstruct the upstream atlas layout for `base..base+0x100` at `cfg` by
/// rendering each codepoint and compositing its (trimmed) bitmap back into the
/// padded grid cell at the offset the trim removed.
fn build_atlas(base: u32, cfg: &SizeCfg) -> Atlas {
    let w = cfg.width;
    let h = cfg.height();
    let pad_x = w / 4;
    let pad_y = h / 4;
    let stride_x = (w + 2 * pad_x) as usize;
    let stride_y = (h + 2 * pad_y) as usize;
    let aw = stride_x * 16;
    let ah = stride_y * 16;
    let mut px = vec![0u8; aw * ah];

    let m = cfg.metrics();
    for idx in 0..0x100u32 {
        let cp = base + idx;
        let Some(g) = render(cp, &m) else { continue };
        if g.width == 0 || g.height == 0 {
            continue;
        }
        // Recover where the trimmed bitmap sits inside the padded cell.
        // From `into_glyph`: offset_x = clip_left - pad_x, and for non-cursor
        // glyphs (draw_height == cell_height) offset_y = (h_region +
        // clip_bottom) - pad_y, so clip_top = buf_h - clip_bottom - h_region.
        let clip_left = g.offset_x + pad_x as i32;
        let clip_bottom = g.offset_y - g.height as i32 + pad_y as i32;
        let clip_top = stride_y as i32 - clip_bottom - g.height as i32;

        let cell_col = (idx % 16) as usize;
        let cell_row = (idx / 16) as usize;
        let cell_x0 = cell_col * stride_x;
        let cell_y0 = cell_row * stride_y;

        for gy in 0..g.height as i32 {
            let ay = cell_y0 as i32 + clip_top + gy;
            if ay < 0 || ay as usize >= ah {
                continue;
            }
            for gx in 0..g.width as i32 {
                let ax = cell_x0 as i32 + clip_left + gx;
                if ax < 0 || ax as usize >= aw {
                    continue;
                }
                let src = g.alpha[(gy as u32 * g.width + gx as u32) as usize];
                px[ay as usize * aw + ax as usize] = src;
            }
        }
    }

    Atlas { w: aw, h: ah, px }
}

/// Decode an upstream golden PNG to grayscale coverage. Upstream writes 8-bit
/// alpha PNGs; the `png` crate surfaces those as `Grayscale` 8-bit. Handle both
/// grayscale and RGBA just in case.
fn load_golden(base: u32, cfg: &SizeCfg) -> Option<Atlas> {
    let name = format!(
        "U+{:X}...U+{:X}-{}x{}+{}.png",
        base,
        base + 0xFF,
        cfg.width,
        cfg.height(),
        cfg.thickness
    );
    let path = format!("{}/tests/testdata/{}", env!("CARGO_MANIFEST_DIR"), name);
    let file = std::fs::File::open(&path).ok()?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let w = info.width as usize;
    let h = info.height as usize;
    let px = match info.color_type {
        png::ColorType::Grayscale => buf[..w * h].to_vec(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2].chunks_exact(2).map(|c| c[0]).collect(),
        png::ColorType::Rgba => buf[..w * h * 4].chunks_exact(4).map(|c| c[3]).collect(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks_exact(3).map(|c| c[0]).collect(),
        _ => return None,
    };
    Some(Atlas { w, h, px })
}

/// Result of comparing one (family, size).
struct DiffStat {
    max_delta: u8,
    diff_pixels: usize,
    total_pixels: usize,
}

impl DiffStat {
    fn diff_fraction(&self) -> f64 {
        if self.total_pixels == 0 {
            0.0
        } else {
            self.diff_pixels as f64 / self.total_pixels as f64
        }
    }
}

fn compare(ours: &Atlas, golden: &Atlas) -> DiffStat {
    assert_eq!(ours.w, golden.w, "atlas width mismatch");
    assert_eq!(ours.h, golden.h, "atlas height mismatch");
    let mut max_delta = 0u8;
    let mut diff_pixels = 0usize;
    for (&a, &b) in ours.px.iter().zip(golden.px.iter()) {
        let d = a.abs_diff(b);
        if d > 0 {
            diff_pixels += 1;
            max_delta = max_delta.max(d);
        }
    }
    DiffStat {
        max_delta,
        diff_pixels,
        total_pixels: ours.px.len(),
    }
}

/// Per-codepoint diff drill-down for one family+size. Run with e.g.
/// `cargo test -p ghostty-sprite --test golden_parity drill -- --ignored --nocapture`.
/// Prints the worst-offending codepoints so a divergence can be localized.
#[test]
#[ignore = "diagnostic; run explicitly to localize a divergence"]
fn drill_down_worst_codepoints() {
    // Default target: the tightest-margin cell. Override via env if desired.
    let base = std::env::var("DRILL_BASE")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x1FB00);
    let cfg = &SIZES[3]; // 9x17+1

    let golden = load_golden(base, cfg).expect("golden present");
    let w = cfg.width;
    let h = cfg.height();
    let pad_x = w / 4;
    let pad_y = h / 4;
    let stride_x = (w + 2 * pad_x) as usize;
    let stride_y = (h + 2 * pad_y) as usize;
    let aw = stride_x * 16;

    let m = cfg.metrics();
    let mut rows: Vec<(u32, usize, u8)> = Vec::new();
    for idx in 0..0x100u32 {
        let cp = base + idx;
        let Some(g) = render(cp, &m) else { continue };
        if g.width == 0 || g.height == 0 {
            continue;
        }
        let clip_left = g.offset_x + pad_x as i32;
        let clip_bottom = g.offset_y - g.height as i32 + pad_y as i32;
        let clip_top = stride_y as i32 - clip_bottom - g.height as i32;
        let cell_x0 = (idx as usize % 16) * stride_x;
        let cell_y0 = (idx as usize / 16) * stride_y;
        let mut diff = 0usize;
        let mut maxd = 0u8;
        for gy in 0..g.height as i32 {
            let ay = cell_y0 as i32 + clip_top + gy;
            for gx in 0..g.width as i32 {
                let ax = cell_x0 as i32 + clip_left + gx;
                if ax < 0 || ay < 0 || ax as usize >= aw || ay as usize >= golden.h {
                    continue;
                }
                let ours = g.alpha[(gy as u32 * g.width + gx as u32) as usize];
                let gold = golden.px[ay as usize * aw + ax as usize];
                let d = ours.abs_diff(gold);
                if d > 0 {
                    diff += 1;
                    maxd = maxd.max(d);
                }
            }
        }
        if diff > 0 {
            rows.push((cp, diff, maxd));
        }
    }
    rows.sort_by_key(|&(_, diff, _)| std::cmp::Reverse(diff));
    println!(
        "\n=== drill U+{:X} @ {}x{}+{} : {} glyphs differ ===",
        base,
        cfg.width,
        cfg.height(),
        cfg.thickness,
        rows.len()
    );
    for (cp, diff, maxd) in rows.iter().take(30) {
        println!("  U+{cp:04X}: {diff:>4} px differ, maxΔ {maxd}");
    }
}

#[test]
fn golden_parity_report() {
    let mut failures = Vec::new();
    // Aggregate per-family worst-case across all sizes for the report table.
    println!("\n=== sprite golden-parity report ===");
    println!(
        "{:<32} {:<9} {:>10} {:>9} {:>9}  verdict",
        "family", "size", "diff%", "maxΔ", "budget%"
    );
    for fam in FAMILIES {
        for cfg in SIZES {
            let golden = match load_golden(fam.base, cfg) {
                Some(g) => g,
                None => {
                    // U+F500/F600 etc. may not have every size; skip gracefully.
                    continue;
                }
            };
            let ours = build_atlas(fam.base, cfg);
            let stat = compare(&ours, &golden);
            let frac = stat.diff_fraction();
            let ok = stat.max_delta <= fam.max_pixel_delta && frac <= fam.max_diff_fraction;
            let size = format!("{}x{}+{}", cfg.width, cfg.height(), cfg.thickness);
            println!(
                "{:<32} {:<9} {:>9.4}% {:>9} {:>8.2}%  {}",
                fam.label,
                size,
                frac * 100.0,
                stat.max_delta,
                fam.max_diff_fraction * 100.0,
                if ok { "PASS" } else { "FAIL" }
            );
            if !ok {
                failures.push(format!(
                    "{} @ {}: diff {:.4}% (budget {:.2}%), maxΔ {} (budget {})",
                    fam.label,
                    size,
                    frac * 100.0,
                    fam.max_diff_fraction * 100.0,
                    stat.max_delta,
                    fam.max_pixel_delta
                ));
            }
        }
    }
    println!();
    assert!(
        failures.is_empty(),
        "sprite golden parity failures:\n  {}",
        failures.join("\n  ")
    );
}

/// Produce the human artifact: `target/sprite-parity.png`, a side-by-side
/// specimen with one row per family (rendered at 18x36+4 for legibility) laid
/// out as three panels — OURS | UPSTREAM | DIFF — where the diff panel paints
/// matching pixels faded gray, pixels only upstream has red, and pixels only we
/// have green (the same visual convention upstream's own `testDiffAtlas` uses).
///
/// This always runs and writes the artifact; it does not assert parity (that is
/// `golden_parity_report`'s job).
#[test]
fn write_specimen_artifact() {
    // Largest size: most legible for a human. 18x36+4.
    let cfg = &SIZES[0];
    let panels: Vec<(&str, Atlas, Option<Atlas>)> = FAMILIES
        .iter()
        .map(|f| (f.label, build_atlas(f.base, cfg), load_golden(f.base, cfg)))
        .collect();

    // Each family's atlas is the same size (16 * stride). Compose vertically,
    // three atlases wide with a small gutter.
    let atlas_w = panels[0].1.w;
    let atlas_h = panels[0].1.h;
    let gutter = 8usize;
    let label_h = 0usize; // labels documented in the report, keep image clean
    let cols = 3;
    let out_w = atlas_w * cols + gutter * (cols + 1);
    let out_h = (atlas_h + gutter) * panels.len() + gutter + label_h * panels.len();

    // RGB output, dark background.
    let mut img = vec![24u8; out_w * out_h * 3];

    let put = |img: &mut [u8], x: usize, y: usize, r: u8, g: u8, b: u8| {
        if x < out_w && y < out_h {
            let i = (y * out_w + x) * 3;
            img[i] = r;
            img[i + 1] = g;
            img[i + 2] = b;
        }
    };

    for (row, (_, ours, golden)) in panels.iter().enumerate() {
        let y0 = gutter + row * (atlas_h + gutter);
        // Panel 1: ours (grayscale coverage on dark).
        let x_ours = gutter;
        // Panel 2: upstream.
        let x_up = gutter * 2 + atlas_w;
        // Panel 3: diff.
        let x_diff = gutter * 3 + atlas_w * 2;
        for y in 0..atlas_h {
            for x in 0..atlas_w {
                let o = ours.px[y * atlas_w + x];
                put(&mut img, x_ours + x, y0 + y, o, o, o);
                match golden {
                    Some(gld) => {
                        let g = gld.px[y * atlas_w + x];
                        put(&mut img, x_up + x, y0 + y, g, g, g);
                        // Diff: match -> faded gray; mismatch -> red(up)/green(ours).
                        if o == g {
                            put(&mut img, x_diff + x, y0 + y, o / 3, o / 3, o / 3);
                        } else {
                            put(&mut img, x_diff + x, y0 + y, g, o, 0);
                        }
                    }
                    None => {
                        put(&mut img, x_up + x, y0 + y, 40, 40, 40);
                        put(&mut img, x_diff + x, y0 + y, o / 3, o / 3, o / 3);
                    }
                }
            }
        }
    }

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/sprite-parity.png"
    );
    let file = std::fs::File::create(path).expect("create artifact");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), out_w as u32, out_h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(&img).expect("png data");
    println!("wrote sprite-parity specimen to {path}");
}
