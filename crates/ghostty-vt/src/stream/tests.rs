//! Stream dispatch tests.
//!
//! Ported from `stream.zig` (dispatch-routing tests, via a spy [`Handler`])
//! and `stream_terminal.zig` (integration tests, via [`TerminalHandler`]).
//! Test names carry the Zig original in a comment. Counts are tracked in
//! `docs/analysis/stream.md`.

use super::*;
use crate::terminal::{Options, Terminal};

// -------------------------------------------------------------------------
// A spy handler that records the last dispatched value per family, to port
// the `stream.zig` routing tests (which use custom comptime `vt` handlers).
// -------------------------------------------------------------------------

#[derive(Default)]
struct Spy {
    cursor_right: Option<u16>,
    set_mode: Option<(Mode, bool)>,
    erase_display: Option<(EraseDisplay, bool)>,
    erase_line: Option<(EraseLine, bool)>,
    cursor_style: Option<CursorStyle>,
    protected: Option<crate::terminal::ProtectedMode>,
    insert_blanks: Option<u16>,
    top_and_bottom_margin: bool,
    restore_cursor: u32,
    tab_clear: Vec<TabClear>,
    tab_set: u32,
    tab_reset: u32,
    sgr_count: usize,
}

impl Handler for Spy {
    fn cursor_right(&mut self, count: u16) {
        self.cursor_right = Some(count);
    }
    fn set_mode(&mut self, mode: Mode, enabled: bool) {
        if enabled {
            self.set_mode = Some((mode, true));
        } else {
            self.set_mode = None;
        }
    }
    fn erase_display(&mut self, mode: EraseDisplay, protected: bool) {
        self.erase_display = Some((mode, protected));
    }
    fn erase_line(&mut self, mode: EraseLine, protected: bool) {
        self.erase_line = Some((mode, protected));
    }
    fn cursor_style(&mut self, style: CursorStyle) {
        self.cursor_style = Some(style);
    }
    fn protected_mode(&mut self, mode: crate::terminal::ProtectedMode) {
        self.protected = Some(mode);
    }
    fn insert_blanks(&mut self, count: u16) {
        self.insert_blanks = Some(count);
    }
    fn top_and_bottom_margin(&mut self, _top: u16, _bottom: u16) {
        self.top_and_bottom_margin = true;
    }
    fn restore_cursor(&mut self) {
        self.restore_cursor += 1;
    }
    fn tab_clear(&mut self, cmd: TabClear) {
        self.tab_clear.push(cmd);
    }
    fn tab_set(&mut self) {
        self.tab_set += 1;
    }
    fn tab_reset(&mut self) {
        self.tab_reset += 1;
    }
    fn set_attribute(&mut self, _attr: sgr::Attribute) {
        self.sgr_count += 1;
    }
}

fn spy(input: &[u8]) -> Spy {
    let mut s = Stream::new(Spy::default());
    s.feed(input);
    s.handler
}

// Zig: "stream: cursor right (CUF)"
#[test]
fn cursor_right_cuf() {
    let mut s = Stream::new(Spy::default());
    s.feed(b"\x1B[C");
    assert_eq!(s.handler.cursor_right, Some(1));

    s.feed(b"\x1B[5C");
    assert_eq!(s.handler.cursor_right, Some(5));

    s.handler.cursor_right = None;
    s.feed(b"\x1B[5;4C");
    assert_eq!(s.handler.cursor_right, None);

    s.handler.cursor_right = None;
    s.feed(b"\x1b[?3C");
    assert_eq!(s.handler.cursor_right, None);
}

// Zig: "stream: dec set mode (SM) and reset mode (RM)"
#[test]
fn dec_set_reset_mode() {
    let mut s = Stream::new(Spy::default());
    s.feed(b"\x1B[?6h");
    assert_eq!(s.handler.set_mode, Some((Mode::Origin, true)));

    s.feed(b"\x1B[?6l");
    assert_eq!(s.handler.set_mode, None);

    s.handler.set_mode = None;
    s.feed(b"\x1B[6 h"); // intermediate space -> invalid
    assert_eq!(s.handler.set_mode, None);
}

// Zig: "stream: ansi set mode (SM) and reset mode (RM)"
#[test]
fn ansi_set_reset_mode() {
    let mut s = Stream::new(Spy::default());
    s.feed(b"\x1B[4h");
    assert_eq!(s.handler.set_mode, Some((Mode::Insert, true)));

    s.feed(b"\x1B[4l");
    assert_eq!(s.handler.set_mode, None);

    s.feed(b"\x1B[>5h"); // '>' intermediate -> not ansi/private, ignored
    assert_eq!(s.handler.set_mode, None);
}

// Zig: "stream: restore mode" (CSI ? 42 r with unknown mode is a no-op).
#[test]
fn restore_mode_unknown_is_noop() {
    let s = spy(b"\x1B[?42r");
    assert!(!s.top_and_bottom_margin);
}

// Zig: "stream: DECED, DECSED"
#[test]
fn deced_decsed() {
    let cases: &[(&[u8], EraseDisplay, bool)] = &[
        (b"\x1B[?J", EraseDisplay::Below, true),
        (b"\x1B[?0J", EraseDisplay::Below, true),
        (b"\x1B[?1J", EraseDisplay::Above, true),
        (b"\x1B[?2J", EraseDisplay::Complete, true),
        (b"\x1B[?3J", EraseDisplay::Scrollback, true),
        (b"\x1B[J", EraseDisplay::Below, false),
        (b"\x1B[0J", EraseDisplay::Below, false),
        (b"\x1B[1J", EraseDisplay::Above, false),
        (b"\x1B[2J", EraseDisplay::Complete, false),
        (b"\x1B[3J", EraseDisplay::Scrollback, false),
    ];
    for (input, mode, prot) in cases {
        let s = spy(input);
        assert_eq!(s.erase_display, Some((*mode, *prot)), "{input:?}");
    }
    // Invalid `>` intermediate: ignored.
    let mut s = Stream::new(Spy::default());
    s.feed(b"\x1B[3J");
    s.feed(b"\x1B[>0J");
    assert_eq!(
        s.handler.erase_display,
        Some((EraseDisplay::Scrollback, false))
    );
}

// Zig: "stream: DECEL, DECSEL"
#[test]
fn decel_decsel() {
    let cases: &[(&[u8], EraseLine, bool)] = &[
        (b"\x1B[?K", EraseLine::Right, true),
        (b"\x1B[?0K", EraseLine::Right, true),
        (b"\x1B[?1K", EraseLine::Left, true),
        (b"\x1B[?2K", EraseLine::Complete, true),
        (b"\x1B[K", EraseLine::Right, false),
        (b"\x1B[0K", EraseLine::Right, false),
        (b"\x1B[1K", EraseLine::Left, false),
        (b"\x1B[2K", EraseLine::Complete, false),
    ];
    for (input, mode, prot) in cases {
        let s = spy(input);
        assert_eq!(s.erase_line, Some((*mode, *prot)), "{input:?}");
    }
    // Invalid `<` intermediate: ignored (last valid state retained).
    let mut s = Stream::new(Spy::default());
    s.feed(b"\x1B[2K");
    s.feed(b"\x1B[<1K");
    assert_eq!(s.handler.erase_line, Some((EraseLine::Complete, false)));
}

// Zig: "stream: DECSCUSR"
#[test]
fn decscusr() {
    assert_eq!(spy(b"\x1B[ q").cursor_style, Some(CursorStyle::Default));
    assert_eq!(
        spy(b"\x1B[1 q").cursor_style,
        Some(CursorStyle::BlinkingBlock)
    );
    assert_eq!(
        spy(b"\x1B[2 q").cursor_style,
        Some(CursorStyle::SteadyBlock)
    );
    assert_eq!(
        spy(b"\x1B[3 q").cursor_style,
        Some(CursorStyle::BlinkingUnderline)
    );
    assert_eq!(
        spy(b"\x1B[4 q").cursor_style,
        Some(CursorStyle::SteadyUnderline)
    );
    assert_eq!(
        spy(b"\x1B[5 q").cursor_style,
        Some(CursorStyle::BlinkingBar)
    );
    assert_eq!(spy(b"\x1B[6 q").cursor_style, Some(CursorStyle::SteadyBar));
}

// Zig: "stream: DECSCUSR without space" — 'q' with no intermediate is not
// DECSCUSR (which requires the space), so no cursor_style fires.
#[test]
fn decscusr_without_space() {
    assert_eq!(spy(b"\x1B[q").cursor_style, None);
}

// Zig: "stream: DECSCA" (CSI Ps " q)
#[test]
fn decsca() {
    use crate::terminal::ProtectedMode;
    assert_eq!(
        spy("\x1B[\"q".as_bytes()).protected,
        Some(ProtectedMode::Off)
    );
    assert_eq!(
        spy("\x1B[0\"q".as_bytes()).protected,
        Some(ProtectedMode::Off)
    );
    assert_eq!(
        spy("\x1B[1\"q".as_bytes()).protected,
        Some(ProtectedMode::Dec)
    );
    assert_eq!(
        spy("\x1B[2\"q".as_bytes()).protected,
        Some(ProtectedMode::Off)
    );
}

// Zig: "stream: insert characters" (ICH)
#[test]
fn insert_characters() {
    assert_eq!(spy(b"\x1B[@").insert_blanks, Some(1));
    assert_eq!(spy(b"\x1B[5@").insert_blanks, Some(5));
}

// Zig: "stream: insert characters explicit zero clamps to 1"
#[test]
fn insert_characters_zero_clamps() {
    assert_eq!(spy(b"\x1B[0@").insert_blanks, Some(1));
}

// Zig: "stream: SCORC" (CSI u with no intermediate -> restore_cursor).
#[test]
fn scorc_route() {
    assert_eq!(spy(b"\x1B[u").restore_cursor, 1);
}

// Zig: "stream: too many csi params" (the whole CSI is dropped).
#[test]
fn too_many_csi_params() {
    let mut input = Vec::from(&b"\x1B["[..]);
    for _ in 0..100 {
        input.extend_from_slice(b"1;");
    }
    input.extend_from_slice(b"1C");
    assert_eq!(spy(&input).cursor_right, None);
}

// Zig: "stream CSI W clear tab stops" / "tab set" / "? W reset tab stops"
#[test]
fn csi_w_tab_ops() {
    assert_eq!(spy(b"\x1B[2W").tab_clear, vec![TabClear::Current]);
    assert_eq!(spy(b"\x1B[5W").tab_clear, vec![TabClear::All]);
    assert_eq!(spy(b"\x1B[W").tab_set, 1);
    assert_eq!(spy(b"\x1B[0W").tab_set, 1);
    assert_eq!(spy(b"\x1B[?5W").tab_reset, 1);
}

// Zig: "stream: tab clear with overflowing param" (invalid, ignored).
#[test]
fn tab_clear_overflow_param() {
    assert!(spy(b"\x1B[99g").tab_clear.is_empty());
}

// Zig: "stream: SGR with 17+ parameters for underline color" — the SGR
// parser is driven with all params and produces attributes.
#[test]
fn sgr_many_params() {
    let s = spy(b"\x1B[4:3;38;2;51;51;51;48;2;170;170;170;58;2;255;97;136m");
    assert!(s.sgr_count > 0);
}

// Zig: "stream: print" — a bare printable char reaches the print handler.
#[test]
fn stream_print() {
    #[derive(Default)]
    struct P {
        last: Option<u32>,
    }
    impl Handler for P {
        fn print(&mut self, cp: u32) {
            self.last = Some(cp);
        }
    }
    let mut s = Stream::new(P::default());
    s.feed(b"x");
    assert_eq!(s.handler.last, Some('x' as u32));
}

// Zig: "simd: print invalid utf-8" — a lone 0xFF prints U+FFFD.
#[test]
fn print_invalid_utf8() {
    #[derive(Default)]
    struct P {
        last: Option<u32>,
    }
    impl Handler for P {
        fn print(&mut self, cp: u32) {
            self.last = Some(cp);
        }
    }
    let mut s = Stream::new(P::default());
    s.feed(&[0xFF]);
    assert_eq!(s.handler.last, Some(0xFFFD));
}

// -------------------------------------------------------------------------
// TerminalHandler integration tests (ported from stream_terminal.zig).
// -------------------------------------------------------------------------

fn term(cols: u16, rows: u16) -> Stream<TerminalHandler> {
    let t = Terminal::new(Options {
        cols,
        rows,
        max_scrollback: 0,
        colors: Default::default(),
    });
    Stream::new(TerminalHandler::new(t))
}

// Zig: "ignores query actions" — DA/DSR/CPR are absorbed and the terminal
// stays functional. (Our engine additionally queues replies.)
#[test]
fn ignores_query_actions() {
    let mut s = term(80, 24);
    s.feed(b"\x1B[c"); // DA
    s.feed(b"\x1B[5n"); // DSR
    s.feed(b"\x1B[6n"); // CPR
    s.feed(b"Test");
    assert_eq!(s.handler.terminal.plain_string(), "Test");
    assert!(!s.handler.output.is_empty());
}

// Zig: "OSC 4 set and reset palette"
#[test]
fn osc4_set_and_reset_palette() {
    let mut s = term(10, 10);
    let default_0 = s.handler.terminal.colors.palette.original[0];

    s.feed(b"\x1b]4;0;rgb:ff/00/00\x1b\\");
    let c = s.handler.terminal.colors.palette.current[0];
    assert_eq!((c.r, c.g, c.b), (0xff, 0x00, 0x00));
    assert!(s.handler.terminal.colors.palette.mask.is_set(0));

    s.feed(b"\x1b]104;0\x1b\\");
    assert_eq!(s.handler.terminal.colors.palette.current[0], default_0);
    assert!(!s.handler.terminal.colors.palette.mask.is_set(0));
}

// Zig: "OSC 104 reset all palette colors"
#[test]
fn osc104_reset_all_palette() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]4;0;rgb:ff/00/00\x1b\\");
    s.feed(b"\x1b]4;1;rgb:00/ff/00\x1b\\");
    s.feed(b"\x1b]4;2;rgb:00/00/ff\x1b\\");
    for i in 0..3 {
        assert!(s.handler.terminal.colors.palette.mask.is_set(i));
    }
    s.feed(b"\x1b]104\x1b\\");
    for i in 0..3 {
        assert_eq!(
            s.handler.terminal.colors.palette.current[i],
            s.handler.terminal.colors.palette.original[i]
        );
        assert!(!s.handler.terminal.colors.palette.mask.is_set(i));
    }
}

// Zig: "OSC 10 set and reset foreground color"
#[test]
fn osc10_fg() {
    let mut s = term(10, 10);
    assert!(s.handler.terminal.colors.foreground.get().is_none());
    s.feed(b"\x1b]10;rgb:ff/00/00\x1b\\");
    let fg = s.handler.terminal.colors.foreground.get().unwrap();
    assert_eq!((fg.r, fg.g, fg.b), (0xff, 0x00, 0x00));
    s.feed(b"\x1b]110\x1b\\");
    assert!(s.handler.terminal.colors.foreground.get().is_none());
}

// Zig: "OSC 11 set and reset background color"
#[test]
fn osc11_bg() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]11;rgb:00/ff/00\x1b\\");
    let bg = s.handler.terminal.colors.background.get().unwrap();
    assert_eq!((bg.r, bg.g, bg.b), (0x00, 0xff, 0x00));
    s.feed(b"\x1b]111\x1b\\");
    assert!(s.handler.terminal.colors.background.get().is_none());
}

// Zig: "OSC 12 set and reset cursor color"
#[test]
fn osc12_cursor() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]12;rgb:00/00/ff\x1b\\");
    let cur = s.handler.terminal.colors.cursor.get().unwrap();
    assert_eq!((cur.r, cur.g, cur.b), (0x00, 0x00, 0xff));
}

// Zig: "kitty color protocol set palette" — OSC 21 is a seam in this chunk
// (the kitty_color handler is a no-op); assert it doesn't corrupt state.
#[test]
fn kitty_color_set_palette_seam() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]21;5=rgb:ff/00/ff\x1b\\");
    s.feed(b"ok");
    assert_eq!(s.handler.terminal.plain_string(), "ok");
}

// OSC 2 window title.
#[test]
fn osc2_window_title() {
    let mut s = term(20, 4);
    s.feed(b"\x1b]2;hello\x07");
    assert_eq!(s.handler.terminal.get_title(), Some(&b"hello"[..]));
}

// OSC 7 report pwd.
#[test]
fn osc7_pwd() {
    let mut s = term(20, 4);
    s.feed(b"\x1b]7;file:///home/user\x07");
    assert_eq!(
        s.handler.terminal.get_pwd(),
        Some(&b"file:///home/user"[..])
    );
}

// DSR cursor position report round-trips the cursor location.
#[test]
fn cpr_reply() {
    let mut s = term(80, 24);
    s.feed(b"\x1B[3;5H"); // move to row 3 col 5 (1-indexed)
    s.feed(b"\x1B[6n"); // CPR
    let out = s.handler.take_output();
    assert_eq!(out, b"\x1B[3;5R");
}

// DECRQSS SGR query reflects the active attributes.
#[test]
fn decrqss_sgr_reply() {
    let mut s = term(20, 4);
    // Default style -> "0".
    s.feed(b"\x1BP$qm\x1B\\");
    assert_eq!(s.handler.take_output(), b"\x1BP1$r0m\x1B\\");

    // Bold + fg red (palette 1).
    s.feed(b"\x1B[1;31m");
    s.feed(b"\x1BP$qm\x1B\\");
    assert_eq!(s.handler.take_output(), b"\x1BP1$r0;1;31m\x1B\\");
}

// Primary device attributes reply.
#[test]
fn da_primary_reply() {
    let mut s = term(20, 4);
    s.feed(b"\x1B[c");
    assert_eq!(s.handler.take_output(), b"\x1b[?62;22c");
}

// -------------------------------------------------------------------------
// Fixture replay against the Rust engine.
// -------------------------------------------------------------------------

/// Decode the `input.esc` escape notation used by the replay fixtures.
/// Mirrors `crates/spike/tests/replay_fixtures.rs::decode_escaped_stream`.
fn decode_escaped_stream(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hi = chars.next().unwrap().to_digit(16).unwrap() as u8;
                let lo = chars.next().unwrap().to_digit(16).unwrap() as u8;
                out.push((hi << 4) | lo);
            }
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.split('\n') {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

fn run_rust_engine(cols: u16, rows: u16, input: &[u8]) -> String {
    let mut s = term(cols, rows);
    s.feed(input);
    normalize(&s.handler.terminal.plain_string())
}

#[test]
fn fixture_prompt_and_color() {
    let bytes = decode_escaped_stream(
        "user@host % printf '\\e[38;5;196mREADY\\e[0m'\\r\\nREADY\\r\\nuser@host %",
    );
    assert_eq!(
        run_rust_engine(40, 6, &bytes),
        "user@host % printf 'READY'\nREADY\nuser@host %"
    );
}

#[test]
fn fixture_alternate_screen_roundtrip() {
    let bytes = decode_escaped_stream("prompt\\r\\n\\e[?1049h\\e[2J\\e[Hvim buffer\\e[?1049lback");
    assert_eq!(run_rust_engine(40, 6, &bytes), "prompt\nback");
}

#[test]
fn fixture_wide_text_and_resize() {
    let bytes = decode_escaped_stream("name: 好\\r\\n\\e[2;7Hok");
    assert_eq!(run_rust_engine(12, 4, &bytes), "name: 好\n      ok");
}
