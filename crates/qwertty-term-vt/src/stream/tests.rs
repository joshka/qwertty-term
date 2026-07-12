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
    print_repeat: Option<u16>,
    kitty_keyboard_pop: Option<u16>,
    kitty_keyboard_push: Option<crate::screen::kitty_key::Flags>,
    kitty_keyboard_set: Option<(
        crate::screen::kitty_key::SetMode,
        crate::screen::kitty_key::Flags,
    )>,
    kitty_keyboard_query: u32,
    title_push: Option<u16>,
    title_pop: Option<u16>,
    mouse_shift_capture: Option<bool>,
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
    fn print_repeat(&mut self, count: u16) {
        self.print_repeat = Some(count);
    }
    fn kitty_keyboard_query(&mut self) {
        self.kitty_keyboard_query += 1;
    }
    fn kitty_keyboard_push(&mut self, flags: crate::screen::kitty_key::Flags) {
        self.kitty_keyboard_push = Some(flags);
    }
    fn kitty_keyboard_pop(&mut self, count: u16) {
        self.kitty_keyboard_pop = Some(count);
    }
    fn kitty_keyboard_set(
        &mut self,
        mode: crate::screen::kitty_key::SetMode,
        flags: crate::screen::kitty_key::Flags,
    ) {
        self.kitty_keyboard_set = Some((mode, flags));
    }
    fn title_push(&mut self, index: u16) {
        self.title_push = Some(index);
    }
    fn title_pop(&mut self, index: u16) {
        self.title_pop = Some(index);
    }
    fn mouse_shift_capture(&mut self, capture: bool) {
        self.mouse_shift_capture = Some(capture);
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

// OSC 52 clipboard write: surfaced as a drainable event, raw (still
// base64-encoded) per upstream's `clipboardContents` policy (decode is an
// apprt/embedder decision, not a terminal-core one).
#[test]
fn osc52_write_is_drainable() {
    let mut s = term(10, 10);
    // "aGVsbG8=" is base64 for "hello"; the terminal-core layer doesn't
    // decode it, just hands it up raw alongside the kind byte.
    s.feed(b"\x1b]52;c;aGVsbG8=\x1b\\");
    assert_eq!(
        s.handler.take_clipboard(),
        Some((b'c', "aGVsbG8=".to_string()))
    );
    // Drained; a second take is empty until another OSC 52 write arrives.
    assert_eq!(s.handler.take_clipboard(), None);
}

// OSC 52 clear (empty data) is still a write event (empty payload), matching
// `clipboard_operation.zig`'s "clear clipboard" case (kind defaults to 'c').
#[test]
fn osc52_clear_is_a_write_with_empty_data() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]52;;\x1b\\");
    assert_eq!(s.handler.take_clipboard(), Some((b'c', String::new())));
}

// OSC 52 query (`?`) is a *read* request, not a write: upstream dispatches a
// distinct `clipboard_read` apprt message and never calls into a write path,
// so this crate's write-event queue stays empty.
#[test]
fn osc52_query_is_not_a_write_event() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]52;s;?\x1b\\");
    assert_eq!(s.handler.take_clipboard(), None);
}

// The clipboard event queue only keeps the most recent write (a UI-facing
// side effect, not a reply queue that must preserve every entry).
#[test]
fn osc52_write_keeps_only_the_latest() {
    let mut s = term(10, 10);
    s.feed(b"\x1b]52;c;Zmlyc3Q=\x1b\\"); // "first"
    s.feed(b"\x1b]52;c;c2Vjb25k\x1b\\"); // "second"
    assert_eq!(
        s.handler.take_clipboard(),
        Some((b'c', "c2Vjb25k".to_string()))
    );
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

// -------------------------------------------------------------------------
// Fast-path equivalence tests (M1 perf pass, docs/analysis/perf.md).
//
// The `feed` decode-until-control-seq bulk path, the batched narrow
// `print_slice` fill, and the bulk CSI-param consume must all be
// behavior-equivalent to feeding the same bytes one at a time (which forces
// the scalar per-byte path). Each test feeds an input three ways and asserts
// identical screen text + cursor:
//   (a) the whole slice in one `feed` (exercises every fast path),
//   (b) one byte per `feed` call (forces the scalar path throughout),
//   (c) awkward chunk splits (exercises fast-path re-entry mid-sequence).
// -------------------------------------------------------------------------

fn snapshot(cols: u16, rows: u16, chunks: &[&[u8]]) -> (String, (u16, u16)) {
    let mut s = term(cols, rows);
    for c in chunks {
        s.feed(c);
    }
    let cur = &s.handler.terminal.screen().cursor;
    (
        normalize(&s.handler.terminal.plain_string()),
        (cur.x, cur.y),
    )
}

fn assert_fastpath_equiv(cols: u16, rows: u16, input: &[u8]) {
    // (a) one shot.
    let whole = snapshot(cols, rows, &[input]);
    // (b) byte-at-a-time.
    let per_byte_chunks: Vec<&[u8]> = input.chunks(1).collect();
    let per_byte = snapshot(cols, rows, &per_byte_chunks);
    assert_eq!(whole, per_byte, "whole-vs-per-byte diverged");
    // (c) a few awkward split sizes to stress fast-path re-entry. Miri is
    // ~100x slower, so it runs one representative split (still exercising the
    // unsafe cell-write path) while the normal runner sweeps several.
    #[cfg(miri)]
    let splits: &[usize] = &[7];
    #[cfg(not(miri))]
    let splits: &[usize] = &[2, 3, 5, 7, 13];
    for &split in splits {
        let chunks: Vec<&[u8]> = input.chunks(split).collect();
        let chunked = snapshot(cols, rows, &chunks);
        assert_eq!(whole, chunked, "whole-vs-chunk({split}) diverged");
    }
}

#[test]
fn fastpath_ascii_soft_wrap() {
    // A run longer than the row width so the batched fill crosses several
    // soft-wrap boundaries (and the pending-wrap column).
    let line = b"The quick brown fox jumps over the lazy dog 0123456789 abcdefghij";
    assert_fastpath_equiv(20, 8, line);
}

#[test]
fn fastpath_ascii_with_c0_controls() {
    // CR/LF/TAB/BS interleaved with printable runs: the ground scan must split
    // C0 (execute) out of the print_slice runs.
    assert_fastpath_equiv(24, 6, b"alpha\tbeta\r\ngamma\x08X delta\r\nend");
}

#[test]
fn fastpath_mixed_utf8_narrow_and_wide() {
    // Mixed narrow UTF-8 + wide CJK + emoji; the narrow batch must stop at
    // wide chars and defer them to the per-cp path.
    assert_fastpath_equiv(16, 6, "héllo wörld 好的 テスト ab🙂cd\r\nx".as_bytes());
}

#[test]
fn fastpath_csi_params_dense() {
    // Many multi-param CSI sequences (SGR + CUP) between short prints: the bulk
    // CSI-param consume path is heavily exercised.
    let chunk =
        b"\x1b[1;31mred\x1b[0m \x1b[38;5;120mpal\x1b[0m \x1b[38;2;10;20;30mrgb\x1b[0m \x1b[4:3m~\x1b[0m\r\n";
    let reps = if cfg!(miri) { 2 } else { 8 };
    let mut input = Vec::new();
    for _ in 0..reps {
        input.extend_from_slice(chunk);
    }
    assert_fastpath_equiv(40, 10, &input);
}

#[test]
fn fastpath_cursor_heavy_moves_and_erase() {
    // CUP + short print + EL, like a full-screen app; exercises CSI-param bulk
    // consume for the row;col params plus batched short prints.
    let mut input = Vec::new();
    let mut row = 1u32;
    let mut col = 1u32;
    let iters = if cfg!(miri) { 12 } else { 60 };
    for _ in 0..iters {
        input.extend_from_slice(format!("\x1b[{row};{col}Hcell\x1b[K").as_bytes());
        row = row % 10 + 1;
        col = (col + 7) % 24 + 1;
    }
    assert_fastpath_equiv(30, 12, &input);
}

#[test]
fn fastpath_private_and_intermediate_csi() {
    // Private-marker (`?`) and intermediate CSI forms must still parse when the
    // bulk param path stops at the non-parameter byte and hands off.
    assert_fastpath_equiv(
        20,
        6,
        b"\x1b[?25lhi\x1b[?25h\x1b[?1049halt\x1b[?1049lmain\x1b[ 1 q done",
    );
}

#[test]
fn fastpath_incomplete_utf8_across_chunks() {
    // A multi-byte codepoint split across a feed boundary: the decoder must
    // carry partial state and the fast path re-enter cleanly. `assert_fastpath_
    // equiv`'s split sizes cut through the 3-byte 好 and 4-byte 🙂.
    assert_fastpath_equiv(12, 4, "ab好cd🙂ef\r\ngh".as_bytes());
}

#[test]
fn fastpath_esc_at_run_boundary() {
    // ESC immediately after a printable run (the common case): scan stops on
    // ESC without consuming it, feed drives the escape via the scalar path.
    assert_fastpath_equiv(20, 5, b"hello\x1bMworld\x1b7save\x1b8");
}

// -------------------------------------------------------------------------
// CSI-entry fast path: true fast-path-vs-state-machine differential.
//
// `assert_fastpath_equiv` above only proves chunking invariance — a single
// byte still enters the fast paths through `feed`. To prove the `csi_entry`
// / `csi_param` fast paths dispatch identically to the pure state machine,
// this drives one terminal through `feed` (fast paths) and a reference
// terminal byte-by-byte through `next`, which routes every non-ground byte
// through `Parser::next` (`next_non_utf8`), bypassing both fast paths.
// -------------------------------------------------------------------------

fn snapshot_via_next(cols: u16, rows: u16, input: &[u8]) -> (String, (u16, u16)) {
    let mut s = term(cols, rows);
    for &b in input {
        s.next(b);
    }
    let cur = &s.handler.terminal.screen().cursor;
    (
        normalize(&s.handler.terminal.plain_string()),
        (cur.x, cur.y),
    )
}

fn assert_fastpath_vs_statemachine(cols: u16, rows: u16, input: &[u8]) {
    let fast = snapshot(cols, rows, &[input]); // feed: csi_entry + csi_param fast paths
    let sm = snapshot_via_next(cols, rows, input); // next per byte: pure Parser::next
    assert_eq!(fast, sm, "fast path diverged from state machine");
}

#[test]
fn csi_entry_parameterless_finals_match_statemachine() {
    // Parameterless finals dispatched straight from csi_entry (no params, no
    // separators): cursor home, erases, SGR reset, save/restore, DECSC-style.
    assert_fastpath_vs_statemachine(
        24,
        6,
        b"ab\x1b[Hx\x1b[Ky\x1b[Jz\x1b[m\x1b[s\x1b[uW\x1b[7mQ\x1b[0m",
    );
}

#[test]
fn csi_entry_digit_and_empty_first_param_match_statemachine() {
    // First-digit path (CUP with two params) and empty-first-param path
    // (`\x1b[;5H`), both starting in csi_entry.
    assert_fastpath_vs_statemachine(
        30,
        10,
        b"\x1b[10;20Hhere\x1b[;5Hthere\x1b[3;3Hx\x1b[;Hcorner",
    );
}

#[test]
fn csi_entry_private_marker_match_statemachine() {
    // Private-marker path (`0x3C..=0x3F` right after `[`): DEC private modes.
    assert_fastpath_vs_statemachine(
        20,
        6,
        b"\x1b[?25lhi\x1b[?25h\x1b[?2004h\x1b[?1049hA\x1b[?1049lB\x1b[>4;2m",
    );
}

#[test]
fn csi_entry_defer_cases_match_statemachine() {
    // Bytes the csi_entry fast path must DEFER to the state machine: an
    // intermediate immediately after `[` (`\x1b[ q` DECSCUSR, space = 0x20),
    // a colon right after `[` (the csi_entry colon edge case), and a C0
    // control mid-entry (cancels the sequence).
    assert_fastpath_vs_statemachine(24, 6, b"\x1b[ qA\x1b[:5mB\x1b[\x18[mC\x1b[!pD");
}

#[test]
fn csi_entry_colon_separator_only_on_m_match_statemachine() {
    // Colon/mixed separators are only honored for the 'm' final; other finals
    // with colon params are dropped. The parameterless-final drop path in
    // csi_entry can't hit this (no params), but csi_param can — verify the
    // combined fast paths agree with the state machine on both.
    assert_fastpath_vs_statemachine(20, 6, b"\x1b[38:2:1:2:3mX\x1b[1:2Hshouldnt-move\x1b[4:3m~");
}

#[test]
fn dropped_final_then_nonascii_ground_byte_match_statemachine() {
    // Regression (found by the feed-vs-state-machine differential fuzz): a CSI
    // that is DROPPED on its final byte (colon separator + non-'m' final, e.g.
    // "ESC [ 7 : l") returns the bulk CSI-param consume to Ground. The byte
    // that follows is a GROUND byte and must go through the UTF-8 decoder, not
    // `next_non_utf8`. A trailing non-ASCII byte (0xB4, a lone UTF-8
    // continuation) must decode to U+FFFD identically on both paths.
    assert_fastpath_vs_statemachine(20, 4, b"\x1b[7:l\xb4");
    // The exact fuzz-minimized input (leading ESC then C1 CSI 0x9B enters
    // csi_entry, then the dropped-final + trailing continuation byte).
    assert_fastpath_vs_statemachine(20, 4, &[0x1b, 0x9b, 0x37, 0x3a, 0x6c, 0xb4]);
    // A few more dropped-final finals with trailing non-ASCII and multi-byte
    // UTF-8, to cover the class rather than the single found input.
    assert_fastpath_vs_statemachine(20, 4, b"\x1b[1:2H\xc3\xa9ok");
    assert_fastpath_vs_statemachine(20, 4, b"\x1b[3:4J\xe4\xbd\xa0z");
}

#[test]
fn csi_entry_max_params_overflow_match_statemachine() {
    // A CSI with more than MAX_PARAMS parameters is dropped entirely; the
    // fast path's overflow rule must match the state machine's.
    let mut input = Vec::new();
    input.extend_from_slice(b"\x1b[");
    for i in 0..40 {
        if i > 0 {
            input.push(b';');
        }
        input.extend_from_slice(b"1");
    }
    input.extend_from_slice(b"mAfter");
    assert_fastpath_vs_statemachine(20, 4, &input);
}

// -------------------------------------------------------------------------
// M1 seam closure: kitty keyboard, XTWINOPS title, XTSHIFTESCAPE, REP.
// Spy-routing tests ported from `stream.zig`; integration tests exercise the
// concrete `TerminalHandler`.
// -------------------------------------------------------------------------

// Zig: "stream: pop kitty keyboard with no params defaults to 1".
#[test]
fn kitty_keyboard_pop_defaults_to_1() {
    let s = spy(b"\x1B[<u");
    assert_eq!(s.kitty_keyboard_pop, Some(1));
}

// Kitty keyboard push routes flags (dispatch-routing coverage; upstream has no
// dedicated push spy test but the dispatch mirrors `stream.zig`'s `'>' u` arm).
#[test]
fn kitty_keyboard_push_routes_flags() {
    let s = spy(b"\x1B[>1u");
    assert_eq!(
        s.kitty_keyboard_push,
        Some(crate::screen::kitty_key::Flags::from_int(1))
    );
    // Default (no params) pushes empty flags.
    let s = spy(b"\x1B[>u");
    assert_eq!(
        s.kitty_keyboard_push,
        Some(crate::screen::kitty_key::Flags::from_int(0))
    );
    // Out-of-range (> u5) is ignored.
    let s = spy(b"\x1B[>99u");
    assert_eq!(s.kitty_keyboard_push, None);
}

// Kitty keyboard set: mode 1=set, 2=or, 3=not (default set).
#[test]
fn kitty_keyboard_set_routes_mode_and_flags() {
    use crate::screen::kitty_key::{Flags, SetMode};
    let s = spy(b"\x1B[=1;1u");
    assert_eq!(
        s.kitty_keyboard_set,
        Some((SetMode::Set, Flags::from_int(1)))
    );
    let s = spy(b"\x1B[=3;2u");
    assert_eq!(
        s.kitty_keyboard_set,
        Some((SetMode::Or, Flags::from_int(3)))
    );
    let s = spy(b"\x1B[=5;3u");
    assert_eq!(
        s.kitty_keyboard_set,
        Some((SetMode::Not, Flags::from_int(5)))
    );
    // No mode param defaults to set.
    let s = spy(b"\x1B[=7u");
    assert_eq!(
        s.kitty_keyboard_set,
        Some((SetMode::Set, Flags::from_int(7)))
    );
}

// Kitty keyboard query routes.
#[test]
fn kitty_keyboard_query_routes() {
    let s = spy(b"\x1B[?u");
    assert_eq!(s.kitty_keyboard_query, 1);
}

// Zig: "stream: XTSHIFTESCAPE".
#[test]
fn xtshiftescape() {
    // Invalid (>=2) is ignored by the handler.
    let s = spy(b"\x1B[>2s");
    assert_eq!(s.mouse_shift_capture, None);
    // No param and 0 both mean false.
    let s = spy(b"\x1B[>s");
    assert_eq!(s.mouse_shift_capture, Some(false));
    let s = spy(b"\x1B[>0s");
    assert_eq!(s.mouse_shift_capture, Some(false));
    // 1 means true.
    let s = spy(b"\x1B[>1s");
    assert_eq!(s.mouse_shift_capture, Some(true));
    // `CSI 1 SP s` is not XTSHIFTESCAPE (intermediate is a space, not `>`); it
    // does not route to mouse_shift_capture.
    let s = spy(b"\x1B[1 s");
    assert_eq!(s.mouse_shift_capture, None);
}

// Zig: "stream: CSI t push title" / "… with explicit window" / "… explicit
// icon" / "… with index".
#[test]
fn csi_t_push_title() {
    // `22;0` → push, index 0.
    assert_eq!(spy(b"\x1b[22;0t").title_push, Some(0));
    // `22;2` (explicit window) → push, index 0.
    assert_eq!(spy(b"\x1b[22;2t").title_push, Some(0));
    // `22;1` (explicit icon only) → NOT dispatched.
    assert_eq!(spy(b"\x1b[22;1t").title_push, None);
    // `22;0;5` → push, index 5.
    assert_eq!(spy(b"\x1b[22;0;5t").title_push, Some(5));
}

// Zig: "stream: CSI t pop title" / "… with explicit window" / "… explicit
// icon" / "… with index".
#[test]
fn csi_t_pop_title() {
    assert_eq!(spy(b"\x1b[23;0t").title_pop, Some(0));
    assert_eq!(spy(b"\x1b[23;2t").title_pop, Some(0));
    assert_eq!(spy(b"\x1b[23;1t").title_pop, None);
    assert_eq!(spy(b"\x1b[23;0;5t").title_pop, Some(5));
}

// Zig: "stream: invalid CSI t" — an unimplemented op (19) routes nowhere. Also
// covers the size-report ops (14/16/18/21) which are a documented seam here
// (they need a pixel-geometry size effect this chunk's Terminal does not own).
#[test]
fn csi_t_seam_ops_route_nowhere() {
    for input in [
        &b"\x1b[19t"[..],
        &b"\x1b[14t"[..],
        &b"\x1b[16t"[..],
        &b"\x1b[18t"[..],
        &b"\x1b[21t"[..],
    ] {
        let s = spy(input);
        assert_eq!(s.title_push, None);
        assert_eq!(s.title_pop, None);
    }
}

// REP (CSI b) routes the repeat count (default 1).
#[test]
fn rep_routes_count() {
    assert_eq!(spy(b"\x1b[b").print_repeat, Some(1));
    assert_eq!(spy(b"\x1b[5b").print_repeat, Some(5));
}

// -------------------------------------------------------------------------
// Integration: TerminalHandler drives real terminal state.
// -------------------------------------------------------------------------

// REP repeats the previously-printed char against the real terminal.
#[test]
fn rep_integration() {
    let mut s = term(10, 5);
    s.feed(b"a\x1b[3b");
    assert_eq!(s.handler.terminal.plain_string(), "aaaa");
}

// Zig: "kitty_keyboard_query" (stream_terminal.zig).
#[test]
fn kitty_keyboard_query_integration() {
    let mut s = term(80, 24);
    // Default flags should be 0.
    s.feed(b"\x1b[?u");
    assert_eq!(s.handler.take_output(), b"\x1b[?0u");
    // Push with the disambiguate flag and query again.
    s.feed(b"\x1b[>1u");
    s.feed(b"\x1b[?u");
    assert_eq!(s.handler.take_output(), b"\x1b[?1u");
}

// XTSHIFTESCAPE records the tri-state on terminal flags.
#[test]
fn xtshiftescape_integration() {
    use crate::terminal::MouseShiftCapture;
    let mut s = term(10, 5);
    s.feed(b"\x1B[>1s");
    assert_eq!(
        s.handler.terminal.flags.mouse_shift_capture,
        MouseShiftCapture::True
    );
    s.feed(b"\x1B[>0s");
    assert_eq!(
        s.handler.terminal.flags.mouse_shift_capture,
        MouseShiftCapture::False
    );
}

// Zig: "kitty graphics APC response" (stream_terminal.zig).
#[test]
fn kitty_graphics_apc_response() {
    let mut s = term(10, 10);
    // Transmit a 1x2 RGB image with id=1 via APC; expect an OK response.
    s.feed(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2,c=10,r=1;////////\x1b\\");
    assert_eq!(s.handler.take_output(), b"\x1b_Gi=1;OK\x1b\\");
}

// Zig: "kitty graphics via APC" (stream_terminal.zig) — the image lands in the
// active screen's storage with the right format.
#[test]
fn kitty_graphics_via_apc() {
    let mut s = term(10, 10);
    s.feed(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2,c=10,r=1;////////\x1b\\");
    let img = s
        .handler
        .terminal
        .screen()
        .kitty_images
        .image_by_id(1)
        .expect("image stored");
    assert_eq!(img.format, crate::kitty::command::Format::Rgb);
}

// End-to-end U=1 (unicode virtual placement): transmit a 2x2 RGB image via
// APC through the real stream, `a=p,U=1` display it as a 2x1-cell virtual
// placement, print two placeholder cells (col 0 and col 1) carrying the
// image id in fg color and the column index via diacritics, then resolve
// the printed cells back into placements and a renderer-facing rect. This
// exercises the full pipeline this chunk ports: APC -> command parse ->
// `kitty::execute` (storage) -> print-path placeholder recognition (row
// flag) -> `kitty::unicode::placement_iterator` -> `render_placement`.
#[test]
fn kitty_unicode_placeholder_end_to_end() {
    use base64::Engine as _;

    let mut s = term(10, 10);
    s.handler.terminal.width_px = 100;
    s.handler.terminal.height_px = 100;

    // Transmit a 2x2 RGB image (id=1), direct medium, uncompressed.
    let pixels = [255u8, 0, 0].repeat(4); // 4 pixels, opaque red.
    let b64 = base64::engine::general_purpose::STANDARD.encode(pixels);
    let transmit = format!("\x1b_Ga=t,t=d,f=24,i=1,s=2,v=2;{b64}\x1b\\");
    s.feed(transmit.as_bytes());
    assert!(
        s.handler
            .terminal
            .screen()
            .kitty_images
            .image_by_id(1)
            .is_some()
    );

    // Display it as a virtual (U=1) placement sized 2 cols x 1 row.
    s.feed(b"\x1b_Ga=p,i=1,U=1,c=2,r=1\x1b\\");
    assert!(
        s.handler
            .terminal
            .screen()
            .kitty_images
            .placements
            .values()
            .any(|p| matches!(p.location, crate::kitty::Location::Virtual))
    );

    // Print the two placeholder cells: fg color 1 = image id 1 (palette);
    // diacritic 1 = row 0; diacritic 2 = col 0 / col 1.
    s.feed(b"\x1b[38;5;1m");
    s.feed("\u{10EEEE}\u{0305}\u{0305}".as_bytes());
    s.feed("\u{10EEEE}\u{0305}\u{030D}".as_bytes());
    s.feed(b"\x1b[39m");

    let t = &s.handler.terminal;
    let pin = t.screen().pages.get_top_left(crate::point::Tag::Viewport);
    let mut it = unsafe { crate::kitty::unicode::placement_iterator(pin, None) };
    let placement = unsafe { it.next() }.expect("one virtual placement run");
    assert!(unsafe { it.next() }.is_none());

    assert_eq!(placement.image_id, 1);
    assert_eq!(placement.placement_id, 0);
    assert_eq!(placement.row, 0);
    assert_eq!(placement.col, 0);
    assert_eq!(placement.width, 2);

    // Resolve into a renderer-facing rect. Cell size: 100px / 10 cols/rows.
    let storage = &t.screen().kitty_images;
    let img = storage.image_by_id(1).unwrap();
    let rp = placement
        .render_placement(storage, img, 10, 10)
        .expect("render placement resolves");
    // The requested grid (2 cols x 1 row -> 20x10px) is wider than the
    // square 2x2 source image, so aspect-fit scales to the grid's height
    // (10px) and letterboxes 5px on each side: a centered 10x10 square.
    assert_eq!(rp.source_width, 2);
    assert_eq!(rp.source_height, 2);
    assert_eq!(rp.dest_width, 10);
    assert_eq!(rp.dest_height, 10);
    assert_eq!(rp.offset_x, 5);
    assert_eq!(rp.offset_y, 0);
}
