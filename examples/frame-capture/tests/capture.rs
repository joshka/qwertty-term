//! Integration tests for the frame-capture example: run the real binary on a
//! scripted session (colors + box drawing + emoji), then assert the PNG
//! exists, has the expected dimensions, contains ink, and is byte-identical
//! across runs (determinism — the property betamax cares about).
//!
//! Skips gracefully (the binary prints `SKIP:` and exits 0) when no Metal
//! device is present, matching the GPU-test convention.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use ghostty_font::Metrics;
use ghostty_font::coretext::Face;

/// The scripted session: a colored prompt, SGR colors, a box-drawing frame,
/// and an emoji.
const SESSION: &str = "\x1b[1;32m$\x1b[0m \x1b[31mred\x1b[0m \x1b[44mblue-bg\x1b[0m\r\n\
                       \u{250c}\u{2500}\u{2500}\u{2510}\r\n\
                       \u{2502}\u{1f600}\u{2502}\r\n\
                       \u{2514}\u{2500}\u{2500}\u{2518}\r\n\
                       done";

const COLS: u16 = 40;
const ROWS: u16 = 6;
const FONT_SIZE: f64 = 16.0;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_frame-capture")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

/// Run the binary; returns stdout, or None if it reported SKIP (no Metal).
fn run(args: &[&str], stdin_path: &Path) -> Option<String> {
    let output = Command::new(bin())
        .args(args)
        .arg(stdin_path)
        .output()
        .expect("spawn frame-capture");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "frame-capture failed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if stdout.contains("SKIP:") {
        eprintln!("SKIP: {}", stdout.trim());
        return None;
    }
    Some(stdout)
}

/// Decode a PNG into (width, height, RGBA bytes).
fn decode_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).expect("open png");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("read png info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png buffer size")];
    let info = reader.next_frame(&mut buf).expect("decode png");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn scripted_session_renders_deterministic_png() {
    let dir = tmp("scripted-session");
    let session = dir.join("session.vt");
    std::fs::write(&session, SESSION.as_bytes()).expect("write session");

    // Run the same capture twice into separate directories.
    let mut pngs = Vec::new();
    for run_dir in ["run1", "run2"] {
        let out_dir = dir.join(run_dir);
        let cols = COLS.to_string();
        let rows = ROWS.to_string();
        let font_size = FONT_SIZE.to_string();
        let args = [
            "--cols",
            &cols,
            "--rows",
            &rows,
            "--font-size",
            &font_size,
            "--out-dir",
            out_dir.to_str().unwrap(),
        ];
        let Some(stdout) = run(&args, &session) else {
            return; // no Metal device: skip
        };
        let png_path = out_dir.join("frame-0000.png");
        assert!(png_path.exists(), "expected {png_path:?}; stdout: {stdout}");
        pngs.push(std::fs::read(&png_path).expect("read png"));

        // Expected dimensions: grid size in cells times the embedded font's
        // cell metrics at the same size, computed through the same public API.
        let face = Face::load_embedded(FONT_SIZE).expect("embedded font");
        let metrics = Metrics::calc(face.face_metrics());
        let (width, height, rgba) = decode_png(&png_path);
        assert_eq!(width, u32::from(COLS) * metrics.cell_width, "png width");
        assert_eq!(height, u32::from(ROWS) * metrics.cell_height, "png height");

        // Ink: a healthy number of pixels differ materially from the default
        // background (0x18 gray) — glyphs, box lines, colored cells.
        let ink = rgba
            .chunks_exact(4)
            .filter(|px| {
                let d = (i32::from(px[0]) - 0x18).abs()
                    + (i32::from(px[1]) - 0x18).abs()
                    + (i32::from(px[2]) - 0x18).abs();
                d > 60
            })
            .count();
        assert!(ink > 100, "expected >100 ink pixels, found {ink}");
    }

    // Determinism: the two runs are byte-identical files.
    assert_eq!(pngs[0], pngs[1], "PNGs from identical runs must match");
}

#[test]
fn marker_splits_into_multiple_frames() {
    let dir = tmp("marker-split");
    let session = dir.join("session.vt");
    // Two screens separated by a File Separator marker (never fed to the
    // terminal): "one", then "two" appended on the next line.
    std::fs::write(&session, b"one\x1c\r\ntwo").expect("write session");

    let out_dir = dir.join("frames");
    let args = [
        "--cols",
        "10",
        "--rows",
        "3",
        "--split-on-marker",
        r"\x1c",
        "--out-dir",
        out_dir.to_str().unwrap(),
    ];
    if run(&args, &session).is_none() {
        return; // no Metal device: skip
    }

    let frame0 = out_dir.join("frame-0000.png");
    let frame1 = out_dir.join("frame-0001.png");
    assert!(frame0.exists(), "first frame (at marker) missing");
    assert!(frame1.exists(), "second frame (at EOF) missing");

    let (w0, h0, px0) = decode_png(&frame0);
    let (w1, h1, px1) = decode_png(&frame1);
    assert_eq!((w0, h0), (w1, h1), "frames share the grid geometry");
    assert_ne!(px0, px1, "the second frame adds a line, so pixels differ");
}
