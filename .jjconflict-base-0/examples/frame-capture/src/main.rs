//! frame-capture: VT bytes in, PNG frames out.
//!
//! A small, headless program proving the qwertty-term embeddability story: feed
//! terminal escape-sequence bytes into a `qwertty_term_vt::Terminal`, render the
//! resulting screen through the `qwertty_term_font` + `qwertty_term_renderer` offscreen
//! stack, and write the pixels out as PNG frames. No window, no pty, no
//! wall-clock — the same input always produces byte-identical PNGs (with the
//! default embedded font).
//!
//! This is the shape of what a terminal recorder like betamax embeds: a
//! terminal state machine plus a deterministic rasterizer, driven entirely by
//! bytes. See README.md in this directory.

/// Non-macOS stub: the offscreen renderer is the Metal backend, so the
/// program follows the GPU-test convention and skips (exit 0) elsewhere.
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("SKIP: frame-capture renders through the Metal backend and requires macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(macos::run());
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use qwertty_term_font::coretext::Face;
    use qwertty_term_font::{CodepointResolver, Collection, Grid, Metrics};
    use qwertty_term_renderer::engine::{Engine, Frame, FrameOptions};
    use qwertty_term_renderer::metal::Metal;
    use qwertty_term_renderer::snapshot::FullSnapshot;
    use qwertty_term_vt::stream::{Stream, TerminalHandler};
    use qwertty_term_vt::terminal::{Options, Terminal};

    const USAGE: &str = "\
frame-capture: VT bytes in, PNG frames out (headless, deterministic)

USAGE:
    frame-capture [OPTIONS] [INPUT]

ARGS:
    [INPUT]    File of raw VT bytes to feed the terminal. `-` or absent: stdin.

OPTIONS:
    --cols <N>                 Terminal width in cells            [default: 80]
    --rows <N>                 Terminal height in cells           [default: 24]
    --font-size <PX>           Font size in pixels                [default: 16]
    --font-family <NAME>       System font family to render with. Default is
                               the embedded JetBrains Mono (deterministic).
    --out-dir <DIR>            Directory for frame PNGs           [default: .]
    --out <FILE>               Exact output path; single-frame (eof) mode only.
    --frame-on <eof|marker>    When to emit a frame               [default: eof]
                               `eof`: one frame after all input is fed.
                               `marker`: a frame at each occurrence of the
                               --split-on-marker byte sequence (the marker
                               bytes themselves are not fed to the terminal),
                               plus a final frame at EOF if bytes follow the
                               last marker.
    --split-on-marker <SEQ>    Marker byte sequence for `marker` mode; implies
                               --frame-on marker. Supports escapes: \\xNN, \\e,
                               \\n, \\r, \\t, \\0, \\\\.
    -h, --help                 Print this help.
";

    struct Args {
        cols: u16,
        rows: u16,
        font_size: f64,
        font_family: Option<String>,
        out_dir: PathBuf,
        out: Option<PathBuf>,
        marker: Option<Vec<u8>>,
        input: Option<PathBuf>,
    }

    pub fn run() -> i32 {
        let args = match parse_args(std::env::args().skip(1)) {
            Ok(Some(args)) => args,
            Ok(None) => {
                print!("{USAGE}");
                return 0;
            }
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!("run with --help for usage");
                return 2;
            }
        };
        match capture(&args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    }

    fn parse_args(mut argv: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
        let mut args = Args {
            cols: 80,
            rows: 24,
            font_size: 16.0,
            font_family: None,
            out_dir: PathBuf::from("."),
            out: None,
            marker: None,
            input: None,
        };
        let mut frame_on: Option<String> = None;

        while let Some(arg) = argv.next() {
            // Accept both `--flag value` and `--flag=value`.
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            let mut value = |name: &str| -> Result<String, String> {
                inline
                    .clone()
                    .or_else(|| argv.next())
                    .ok_or_else(|| format!("{name} requires a value"))
            };
            match flag.as_str() {
                "-h" | "--help" => return Ok(None),
                "--cols" => {
                    args.cols = value("--cols")?
                        .parse()
                        .map_err(|e| format!("--cols: {e}"))?;
                }
                "--rows" => {
                    args.rows = value("--rows")?
                        .parse()
                        .map_err(|e| format!("--rows: {e}"))?;
                }
                "--font-size" => {
                    args.font_size = value("--font-size")?
                        .parse()
                        .map_err(|e| format!("--font-size: {e}"))?;
                }
                "--font-family" => args.font_family = Some(value("--font-family")?),
                "--out-dir" => args.out_dir = PathBuf::from(value("--out-dir")?),
                "--out" => args.out = Some(PathBuf::from(value("--out")?)),
                "--frame-on" => frame_on = Some(value("--frame-on")?),
                "--split-on-marker" => {
                    let seq = unescape(&value("--split-on-marker")?)?;
                    if seq.is_empty() {
                        return Err("--split-on-marker: marker must be non-empty".into());
                    }
                    args.marker = Some(seq);
                }
                _ if flag.starts_with('-') && flag != "-" => {
                    return Err(format!("unknown option {flag}"));
                }
                _ => {
                    if args.input.is_some() {
                        return Err(format!("unexpected extra argument {arg}"));
                    }
                    if arg != "-" {
                        args.input = Some(PathBuf::from(arg));
                    } else {
                        // `-` means stdin, which is also the default.
                    }
                }
            }
        }

        // Cross-validate frame mode.
        match frame_on.as_deref() {
            None => {}
            Some("eof") => {
                if args.marker.is_some() {
                    return Err("--frame-on eof conflicts with --split-on-marker".into());
                }
            }
            Some("marker") => {
                if args.marker.is_none() {
                    return Err("--frame-on marker requires --split-on-marker".into());
                }
            }
            Some(other) => return Err(format!("--frame-on: expected eof|marker, got {other}")),
        }
        if args.out.is_some() && args.marker.is_some() {
            return Err("--out is for single-frame (eof) mode; use --out-dir with markers".into());
        }
        if args.cols == 0 || args.rows == 0 {
            return Err("--cols and --rows must be at least 1".into());
        }
        Ok(Some(args))
    }

    /// Decode a marker argument: `\xNN`, `\e`, `\n`, `\r`, `\t`, `\0`, `\\`;
    /// everything else passes through as UTF-8 bytes.
    fn unescape(s: &str) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                continue;
            }
            match chars.next() {
                Some('x') => {
                    let hi = chars.next().and_then(|c| c.to_digit(16));
                    let lo = chars.next().and_then(|c| c.to_digit(16));
                    match (hi, lo) {
                        (Some(hi), Some(lo)) => out.push((hi * 16 + lo) as u8),
                        _ => return Err(r"invalid \x escape (expected two hex digits)".into()),
                    }
                }
                Some('e') => out.push(0x1b),
                Some('n') => out.push(b'\n'),
                Some('r') => out.push(b'\r'),
                Some('t') => out.push(b'\t'),
                Some('0') => out.push(0),
                Some('\\') => out.push(b'\\'),
                other => return Err(format!(r"unsupported escape \{}", other.unwrap_or(' '))),
            }
        }
        Ok(out)
    }

    /// Split `input` at each occurrence of `marker`. Every returned segment
    /// except possibly the last was terminated by a marker; the marker bytes
    /// are consumed by the split (never fed to the terminal).
    fn split_on_marker<'a>(input: &'a [u8], marker: &[u8]) -> Vec<&'a [u8]> {
        let mut segments = Vec::new();
        let mut rest = input;
        while let Some(pos) = find(rest, marker) {
            segments.push(&rest[..pos]);
            rest = &rest[pos + marker.len()..];
        }
        segments.push(rest);
        segments
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn capture(args: &Args) -> Result<(), String> {
        // --- Input bytes: file or stdin, all up front (no timing anywhere). ---
        let input = match &args.input {
            Some(path) => std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
            None => {
                let mut buf = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("stdin: {e}"))?;
                buf
            }
        };

        // --- GPU: skip gracefully with no Metal device (CI, containers). ---
        let backend = match Metal::new() {
            Ok(b) => b,
            Err(e) => {
                println!("SKIP: no Metal device ({e}); frame-capture needs a Metal-capable GPU");
                return Ok(());
            }
        };

        // --- Font substrate: embedded JetBrains Mono by default so the same
        //     input yields byte-identical pixels on any machine. ---
        let face = match &args.font_family {
            None => {
                Face::load_embedded(args.font_size).map_err(|e| format!("embedded font: {e}"))?
            }
            Some(name) => Face::load_by_name(name, args.font_size)
                .map_err(|e| format!("font family {name:?}: {e}"))?,
        };
        let metrics = Metrics::calc(face.face_metrics());
        let resolver = CodepointResolver::new(Collection::new(face));
        let mut grid = Grid::new(resolver, metrics).map_err(|e| format!("font grid: {e}"))?;

        // --- Terminal + engine. The engine reads its cell geometry from the
        //     grid, so the two can't disagree. ---
        let terminal = Terminal::new(Options {
            cols: args.cols,
            rows: args.rows,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(terminal));
        let mut engine =
            Engine::with_backend_for_grid(backend, &grid).map_err(|e| format!("engine: {e}"))?;

        std::fs::create_dir_all(&args.out_dir)
            .map_err(|e| format!("{}: {e}", args.out_dir.display()))?;

        // --- Feed and emit frames. ---
        let segments = match &args.marker {
            Some(marker) => split_on_marker(&input, marker),
            None => vec![&input[..]],
        };
        let last = segments.len() - 1;
        // Every iteration before a `break` emits exactly one frame, so the
        // segment index doubles as the frame index.
        for (i, segment) in segments.iter().enumerate() {
            stream.feed(segment);
            // Marker mode: the final segment is EOF-terminated, not
            // marker-terminated; emit it only if it added bytes (or nothing
            // was emitted at all, so there is always at least one frame).
            if args.marker.is_some() && i == last && segment.is_empty() && i > 0 {
                break;
            }
            let path = match &args.out {
                Some(path) => path.clone(),
                None => args.out_dir.join(format!("frame-{i:04}.png")),
            };
            render_frame(&mut engine, stream.terminal(), &mut grid, &path)?;
        }
        Ok(())
    }

    /// Snapshot the terminal, render one offscreen frame, and write it as a
    /// PNG. This function is the whole embeddability story: capture the live
    /// screen, one `render` call, encode the pixels.
    fn render_frame(
        engine: &mut Engine,
        terminal: &Terminal,
        grid: &mut Grid,
        path: &Path,
    ) -> Result<(), String> {
        let snapshot = FullSnapshot::capture_live(terminal);
        let frame = engine
            .render(&snapshot, grid, FrameOptions::default())
            .map_err(|e| format!("render: {e}"))?;
        write_png(path, &frame)?;
        println!(
            "wrote {} ({}x{} px)",
            path.display(),
            frame.width(),
            frame.height()
        );
        Ok(())
    }

    /// Write a rendered frame as an RGBA PNG.
    fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let file = std::fs::File::create(path).map_err(|e| err(&e))?;
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(file),
            frame.width() as u32,
            frame.height() as u32,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| err(&e))?;
        writer
            .write_image_data(&frame.to_rgba())
            .map_err(|e| err(&e))?;
        writer.finish().map_err(|e| err(&e))?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{split_on_marker, unescape};

        #[test]
        fn unescape_decodes_common_escapes() {
            assert_eq!(unescape(r"\x1b[0m\n").unwrap(), b"\x1b[0m\n");
            assert_eq!(unescape(r"\e\r\t\0\\").unwrap(), b"\x1b\r\t\0\\");
            assert_eq!(unescape("plain").unwrap(), b"plain");
            assert!(unescape(r"\q").is_err());
            assert!(unescape(r"\x1").is_err());
        }

        #[test]
        fn split_consumes_marker_bytes() {
            assert_eq!(
                split_on_marker(b"one\x1ctwo", b"\x1c"),
                vec![&b"one"[..], &b"two"[..]]
            );
            assert_eq!(
                split_on_marker(b"tail\x1c", b"\x1c"),
                vec![&b"tail"[..], &b""[..]]
            );
            assert_eq!(
                split_on_marker(b"nomarker", b"\x1c"),
                vec![&b"nomarker"[..]]
            );
        }
    }
}
