//! Tests ported from `src/terminal/formatter.zig` inline tests (ghostty
//! `2da015cd6`). See `docs/analysis/formatter.md` for the mapping.
//!
//! ## Port strategy
//!
//! Upstream has **100** inline tests, most at the `PageFormatter` level built
//! on a raw `Page` with `formatWithState` + `point_map`/subset ranges. The Rust
//! port drives off `Screen`/`Terminal` read-back (there is no raw-`Page` test
//! builder in the port), so those tests are re-expressed as `Terminal`-driven
//! tests asserting the same **formatted bytes**. `point_map`/`pin_map`
//! assertions are dropped (that byte→pin tracking is a deferred render feature —
//! see the analysis doc); the text assertions are kept verbatim.
//!
//! Tests that require arbitrary page subsets / rectangle selections / genuine
//! multi-page splits / OSC8 HTML anchors are deferred to their owning chunks
//! (selection, hyperlink); see the module tail for the exact list.

use super::*;
use crate::color::Rgb;
use crate::stream::{Stream, TerminalHandler};
use crate::terminal::{Options as TermOptions, Terminal};

fn feed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let term = Terminal::new(TermOptions {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(bytes);
    stream.handler.terminal
}

/// Raw plain options (trim=false), matching Zig's `Options{ .emit = .plain }`
/// default before the `.plain` preset sets trim.
fn plain_notrim() -> Options {
    Options {
        emit: FormatOpt(Format::Plain),
        trim: false,
        ..Default::default()
    }
}

fn vt_notrim() -> Options {
    Options {
        emit: FormatOpt(Format::Vt),
        trim: false,
        ..Default::default()
    }
}

fn html_notrim() -> Options {
    Options {
        emit: FormatOpt(Format::Html),
        trim: false,
        ..Default::default()
    }
}

fn fmt(t: &Terminal, opts: &Options) -> String {
    t.format(opts, &TerminalExtra::none())
}

// ===========================================================================
// Plain text (Page/Screen/Terminal plain family)
// ===========================================================================

// "Page plain single line"
#[test]
fn plain_single_line() {
    let t = feed(80, 24, b"hello, world");
    assert_eq!(fmt(&t, &plain_notrim()), "hello, world");
}

// "Page plain multiline"
#[test]
fn plain_multiline() {
    let t = feed(80, 24, b"hello\r\nworld");
    assert_eq!(fmt(&t, &plain_notrim()), "hello\nworld");
}

// "Page plain multi blank lines"
#[test]
fn plain_multi_blank_lines() {
    let t = feed(80, 24, b"hello\r\n\r\n\r\nworld");
    assert_eq!(fmt(&t, &plain_notrim()), "hello\n\n\nworld");
}

// "Page plain trailing blank lines" — trailing blank rows always dropped.
#[test]
fn plain_trailing_blank_lines() {
    let t = feed(80, 24, b"hello\r\nworld\r\n\r\n");
    assert_eq!(fmt(&t, &plain_notrim()), "hello\nworld");
}

// "Page plain trailing whitespace" (trim on): trailing spaces removed.
#[test]
fn plain_trailing_whitespace_trim() {
    let t = feed(80, 24, b"hello   \r\nworld  ");
    let opts = Options::plain(); // trim = true
    assert_eq!(fmt(&t, &opts), "hello\nworld");
}

// "Page plain trailing whitespace no trim": spaces preserved.
#[test]
fn plain_trailing_whitespace_no_trim() {
    let t = feed(80, 24, b"hello   \r\nworld  ");
    assert_eq!(fmt(&t, &plain_notrim()), "hello   \nworld  ");
}

// "Page plain single wide char"
#[test]
fn plain_single_wide_char() {
    let t = feed(80, 24, "1A⚡".as_bytes());
    assert_eq!(fmt(&t, &plain_notrim()), "1A⚡");
}

// "Page plain single line soft-wrapped unwrapped" family:
// soft-wrap without unwrap keeps the physical break, with unwrap joins.
#[test]
fn plain_soft_wrapped_without_unwrap() {
    // 10 cols: "hello worl" wraps to "d test".
    let t = feed(10, 5, b"hello world test");
    assert_eq!(fmt(&t, &Options::plain()), "hello worl\nd test");
}

#[test]
fn plain_soft_wrapped_with_unwrap() {
    let t = feed(10, 5, b"hello world test");
    let opts = Options {
        emit: FormatOpt(Format::Plain),
        unwrap: true,
        ..Default::default()
    };
    assert_eq!(fmt(&t, &opts), "hello world test");
}

#[test]
fn plain_soft_wrapped_3_lines_without_unwrap() {
    let t = feed(10, 5, b"hello world this is a test");
    assert_eq!(fmt(&t, &Options::plain()), "hello worl\nd this is\na test");
}

#[test]
fn plain_soft_wrapped_3_lines_with_unwrap() {
    let t = feed(10, 5, b"hello world this is a test");
    let opts = Options {
        emit: FormatOpt(Format::Plain),
        unwrap: true,
        ..Default::default()
    };
    assert_eq!(fmt(&t, &opts), "hello world this is a test");
}

// ===========================================================================
// VT styled output (Page/Screen VT family)
// ===========================================================================

// "Page VT single line plain text"
#[test]
fn vt_single_line_plain_text() {
    let t = feed(80, 24, b"hello");
    assert_eq!(fmt(&t, &vt_notrim()), "hello");
}

// "Page VT single line with bold"
#[test]
fn vt_single_line_with_bold() {
    let t = feed(80, 24, b"\x1b[1mhello\x1b[0m");
    assert_eq!(fmt(&t, &vt_notrim()), "\x1b[0m\x1b[1mhello\x1b[0m");
}

// "Page VT multiple styles"
#[test]
fn vt_multiple_styles() {
    let t = feed(80, 24, b"\x1b[1mhello \x1b[3mworld\x1b[0m");
    assert_eq!(
        fmt(&t, &vt_notrim()),
        "\x1b[0m\x1b[1mhello \x1b[0m\x1b[1m\x1b[3mworld\x1b[0m"
    );
}

// "Page VT with foreground color"
#[test]
fn vt_with_foreground_color() {
    let t = feed(80, 24, b"\x1b[31mred\x1b[0m");
    assert_eq!(fmt(&t, &vt_notrim()), "\x1b[0m\x1b[38;5;1mred\x1b[0m");
}

// "Page VT multi-line with styles"
#[test]
fn vt_multi_line_with_styles() {
    let t = feed(80, 24, b"\x1b[1mfirst\x1b[0m\r\n\x1b[3msecond\x1b[0m");
    assert_eq!(
        fmt(&t, &vt_notrim()),
        "\x1b[0m\x1b[1mfirst\x1b[0m\r\n\x1b[0m\x1b[3msecond\x1b[0m"
    );
}

// "Page VT duplicate style not emitted twice" / "reset properly closes styles"
#[test]
fn vt_style_reset_closes_styles() {
    let t = feed(80, 24, b"\x1b[1mbold\x1b[0mnormal");
    assert_eq!(fmt(&t, &vt_notrim()), "\x1b[0m\x1b[1mbold\x1b[0mnormal");
}

// "Page VT with palette option emits RGB" (palette=null → indices)
#[test]
fn vt_palette_none_emits_index() {
    let t = feed(80, 24, b"\x1b[31mred\x1b[0m");
    assert_eq!(fmt(&t, &vt_notrim()), "\x1b[0m\x1b[38;5;1mred\x1b[0m");
}

// "Page VT with palette option emits RGB" (palette set → RGB). Palette index 1
// is the default red 0xcc6666 → but the Zig test explicitly builds a palette
// where index 1 = ab/cd/ef. We mirror that by supplying a palette.
#[test]
fn vt_palette_set_emits_rgb() {
    let t = feed(80, 24, b"\x1b[31mred\x1b[0m");
    let mut palette = crate::color::DynamicPalette::DEFAULT.current;
    palette[1] = Rgb {
        r: 0xab,
        g: 0xcd,
        b: 0xef,
    };
    let opts = Options {
        emit: FormatOpt(Format::Vt),
        palette: Some(palette),
        ..Default::default()
    };
    assert_eq!(fmt(&t, &opts), "\x1b[0m\x1b[38;2;171;205;239mred\x1b[0m");
}

// "Page VT background color on trailing blank cells" — the bg SGR must appear
// before the newline, not lost.
#[test]
fn vt_bg_color_on_trailing_blank_cells() {
    let t = feed(20, 5, b"CPU:\x1b[41m\x1b[K\x1b[0m\r\nline2");
    let out = fmt(&t, &vt_notrim());
    let crlf = out.find("\r\n").expect("CRLF present");
    let line1 = &out[..crlf];
    assert!(
        line1.contains("\x1b[41m") || line1.contains("\x1b[48;5;1m"),
        "red bg on line 1: {line1:?}"
    );
}

// ===========================================================================
// HTML output (Page HTML family)
// ===========================================================================

// "Page html plain text"
#[test]
fn html_plain_text() {
    let t = feed(80, 24, b"hello, world");
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">hello, world</div>"
    );
}

// "Page html with multiple styles"
#[test]
fn html_with_multiple_styles() {
    let t = feed(80, 24, b"\x1b[1mbold\x1b[3mitalic\x1b[0mnormal");
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">\
            <div style=\"display: inline;font-weight: bold;\">bold</div>\
            <div style=\"display: inline;font-weight: bold;font-style: italic;\">italic</div>\
            normal\
            </div>"
    );
}

// "Page html with colors"
#[test]
fn html_with_colors() {
    let t = feed(80, 24, b"\x1b[31;44mcolored");
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">\
            <div style=\"display: inline;color: var(--vt-palette-1);background-color: var(--vt-palette-4);\">colored</div>\
            </div>"
    );
}

// "Page html with background and foreground colors"
#[test]
fn html_with_bg_and_fg_colors() {
    let t = feed(80, 24, b"hello");
    let opts = Options {
        emit: FormatOpt(Format::Html),
        background: Some(Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        }),
        foreground: Some(Rgb {
            r: 0xab,
            g: 0xcd,
            b: 0xef,
        }),
        ..Default::default()
    };
    assert_eq!(
        fmt(&t, &opts),
        "<div style=\"font-family: monospace; white-space: pre;background-color: #123456;color: #abcdef;\">hello</div>"
    );
}

// "Page html with escaping"
#[test]
fn html_with_escaping() {
    let t = feed(80, 24, b"<tag>&\"'text");
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">&lt;tag&gt;&amp;&quot;&#39;text</div>"
    );
}

// "Page html ascii characters unchanged"
#[test]
fn html_ascii_unchanged() {
    let t = feed(80, 24, b"Hello123");
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">Hello123</div>"
    );
}

// "Page html with unicode as numeric entities"
#[test]
fn html_unicode_numeric_entities() {
    let t = feed(80, 24, "café".as_bytes());
    // 'é' = U+00E9 = 233
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">caf&#233;</div>"
    );
}

// "Page html mixed ascii and unicode"
#[test]
fn html_mixed_ascii_unicode() {
    let t = feed(80, 24, "a⚡b".as_bytes());
    // '⚡' = U+26A1 = 9889
    assert_eq!(
        fmt(&t, &html_notrim()),
        "<div style=\"font-family: monospace; white-space: pre;\">a&#9889;b</div>"
    );
}

// ===========================================================================
// codepoint_map (Page codepoint_map family)
// ===========================================================================

fn cp_map(range: (char, char), repl: Replacement) -> CodepointMap {
    CodepointMap {
        range: (u32::from(range.0), u32::from(range.1)),
        replacement: repl,
    }
}

// "Page codepoint_map single replacement"
#[test]
fn codepoint_map_single_replacement() {
    let t = feed(80, 24, b"hello world");
    let opts = Options {
        codepoint_map: vec![cp_map(('o', 'o'), Replacement::Codepoint('x'))],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "hellx wxrld");
}

// "Page codepoint_map conflicting replacement prefers last"
#[test]
fn codepoint_map_prefers_last() {
    let t = feed(80, 24, b"hello");
    let opts = Options {
        codepoint_map: vec![
            cp_map(('o', 'o'), Replacement::Codepoint('x')),
            cp_map(('o', 'o'), Replacement::Codepoint('y')),
        ],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "helly");
}

// "Page codepoint_map replace with string"
#[test]
fn codepoint_map_replace_with_string() {
    let t = feed(80, 24, b"hello");
    let opts = Options {
        codepoint_map: vec![cp_map(('o', 'o'), Replacement::Str("XYZ".into()))],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "hellXYZ");
}

// "Page codepoint_map range replacement"
#[test]
fn codepoint_map_range_replacement() {
    let t = feed(80, 24, b"abcdefg");
    let opts = Options {
        codepoint_map: vec![cp_map(('b', 'e'), Replacement::Codepoint('X'))],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "aXXXXfg");
}

// "Page codepoint_map multiple ranges"
#[test]
fn codepoint_map_multiple_ranges() {
    let t = feed(80, 24, b"hello world");
    let opts = Options {
        codepoint_map: vec![
            cp_map(('a', 'm'), Replacement::Codepoint('A')),
            cp_map(('n', 'z'), Replacement::Codepoint('Z')),
        ],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "AAAAZ ZZZAA");
}

// "Page codepoint_map unicode replacement"
#[test]
fn codepoint_map_unicode_replacement() {
    let t = feed(80, 24, "hello ⚡ world".as_bytes());
    let opts = Options {
        codepoint_map: vec![cp_map(('⚡', '⚡'), Replacement::Str("🔥".into()))],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "hello 🔥 world");
}

// "Page codepoint_map with styled formats"
#[test]
fn codepoint_map_with_styled_formats() {
    let t = feed(80, 24, b"\x1b[31mred text\x1b[0m");
    let opts = Options {
        emit: FormatOpt(Format::Vt),
        codepoint_map: vec![cp_map(('e', 'e'), Replacement::Codepoint('X'))],
        ..Default::default()
    };
    assert_eq!(fmt(&t, &opts), "\x1b[0m\x1b[38;5;1mrXd tXxt\x1b[0m");
}

// "Page codepoint_map empty map"
#[test]
fn codepoint_map_empty() {
    let t = feed(80, 24, b"hello world");
    let opts = Options {
        codepoint_map: vec![],
        ..plain_notrim()
    };
    assert_eq!(fmt(&t, &opts), "hello world");
}

// ===========================================================================
// TerminalFormatter / palette / modes / region / tabstops / pwd
// ===========================================================================

// "TerminalFormatter plain no selection"
#[test]
fn terminal_plain_no_selection() {
    let t = feed(80, 24, b"hello\r\nworld");
    assert_eq!(
        t.format(&Options::plain(), &TerminalExtra::none()),
        "hello\nworld"
    );
}

// "TerminalFormatter with selection" / "Screen plain with selection":
// selecting active row 1, cols 0..=4 → "line2".
#[test]
fn terminal_with_selection() {
    let t = feed(80, 24, b"line1\r\nline2\r\nline3");
    let tl = Point::new(Tag::Active, crate::point::Coordinate { x: 0, y: 1 });
    let br = Point::new(Tag::Active, crate::point::Coordinate { x: 4, y: 1 });
    let out = t.format_content(
        &Options::plain(),
        &TerminalExtra::none(),
        Content::Range { tl, br },
    );
    assert_eq!(out, "line2");
}

// "TerminalFormatter vt with palette": round-trips palette through a 2nd term.
#[test]
fn terminal_vt_with_palette_roundtrip() {
    let t = feed(
        80,
        24,
        b"\x1b]4;0;rgb:12/34/56\x1b\\\x1b]4;1;rgb:ab/cd/ef\x1b\\\x1b]4;255;rgb:ff/00/ff\x1b\\test",
    );
    let out = t.format(&Options::vt(), &TerminalExtra::styles());
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(t.colors.palette.current[0], t2.colors.palette.current[0]);
    assert_eq!(t.colors.palette.current[1], t2.colors.palette.current[1]);
    assert_eq!(
        t.colors.palette.current[255],
        t2.colors.palette.current[255]
    );
}

// "TerminalFormatter html with palette": CSS variables emitted.
#[test]
fn terminal_html_with_palette() {
    let t = feed(
        80,
        24,
        b"\x1b]4;0;rgb:12/34/56\x1b\\\x1b]4;1;rgb:ab/cd/ef\x1b\\\x1b]4;255;rgb:ff/00/ff\x1b\\test",
    );
    let extra = TerminalExtra {
        palette: true,
        ..Default::default()
    };
    let out = t.format(&html_notrim(), &extra);
    assert!(out.contains("<style>:root{"));
    assert!(out.contains("--vt-palette-0: #123456;"));
    assert!(out.contains("--vt-palette-1: #abcdef;"));
    assert!(out.contains("--vt-palette-255: #ff00ff;"));
    assert!(out.contains("}</style>"));
    assert!(out.contains("test"));
}

// "Terminal vt with scrolling region": round-trip DECSTBM.
#[test]
fn terminal_vt_scrolling_region() {
    let t = feed(80, 24, b"\x1b[5;20r");
    let extra = TerminalExtra {
        scrolling_region: true,
        ..Default::default()
    };
    let out = t.format(&Options::vt(), &extra);
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(t.scrolling_region.top, t2.scrolling_region.top);
    assert_eq!(t.scrolling_region.bottom, t2.scrolling_region.bottom);
}

// "Terminal vt with modes": round-trip a non-default mode.
#[test]
fn terminal_vt_modes() {
    // Enable bracketed paste (2004) which defaults off.
    let t = feed(80, 24, b"\x1b[?2004h");
    let extra = TerminalExtra {
        modes: true,
        ..Default::default()
    };
    let out = t.format(&Options::vt(), &extra);
    assert!(out.contains("\x1b[?2004h"), "modes emit: {out:?}");
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(
        t.modes.get(crate::modes::Mode::BracketedPaste),
        t2.modes.get(crate::modes::Mode::BracketedPaste)
    );
}

// "Terminal vt with tabstops": clear + set stops via HTS.
#[test]
fn terminal_vt_tabstops() {
    let t = feed(80, 24, b"");
    let extra = TerminalExtra {
        tabstops: true,
        ..Default::default()
    };
    let out = t.format(&Options::vt(), &extra);
    assert!(out.starts_with("\x1b[3g"), "tabstops emit: {out:?}");
    let t2 = feed(80, 24, out.as_bytes());
    // Default tabstops are every 8 cols; the reconstruction should match.
    for col in 0..80usize {
        assert_eq!(t.tabstops.get(col), t2.tabstops.get(col), "col {col}");
    }
}

// "Terminal vt with keyboard modes": modifyOtherKeys. The `\x1b[>4;2m` CSI
// isn't wired into the Rust stream yet (a Terminal chunk item), so we set the
// flag directly to exercise the *formatter's* emission (the unit under test).
#[test]
fn terminal_vt_keyboard_modes() {
    let mut t = feed(80, 24, b"");
    t.flags.modify_other_keys_2 = true;
    let extra = TerminalExtra {
        keyboard: true,
        ..Default::default()
    };
    let out = t.format(&Options::vt(), &extra);
    assert!(out.contains("\x1b[>4;2m"), "keyboard emit: {out:?}");
}

// "Terminal vt with pwd": OSC 7.
#[test]
fn terminal_vt_pwd() {
    let t = feed(80, 24, b"\x1b]7;file:///home/user\x1b\\");
    let extra = TerminalExtra {
        pwd: true,
        ..Default::default()
    };
    let out = t.format(&Options::vt(), &extra);
    assert!(
        out.contains("\x1b]7;file:///home/user\x1b\\"),
        "pwd emit: {out:?}"
    );
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(t.pwd, t2.pwd);
}

// "Screen vt with cursor position": CUP round-trip.
#[test]
fn screen_vt_cursor_position() {
    let t = feed(80, 24, b"hello\r\nworld");
    let extra = ScreenExtra {
        cursor: true,
        ..Default::default()
    };
    let out = t.screen().format(&Options::vt(), &extra, Content::All);
    // Cursor at row 1 (0-idx), col 5 → CUP "\x1b[2;6H".
    assert!(out.contains("\x1b[2;6H"), "cursor emit: {out:?}");
}

// "Screen vt with style": cursor style round-trip.
#[test]
fn screen_vt_style() {
    let t = feed(80, 24, b"\x1b[1;31mhello");
    let extra = ScreenExtra {
        style: true,
        ..Default::default()
    };
    let out = t.screen().format(&Options::vt(), &extra, Content::All);
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(t.screen().cursor.style, t2.screen().cursor.style);
}

// "Screen vt with protection": DECSCA round-trip.
#[test]
fn screen_vt_protection() {
    let t = feed(80, 24, b"\x1b[1\"qhello");
    let extra = ScreenExtra {
        protection: true,
        ..Default::default()
    };
    let out = t.screen().format(&Options::vt(), &extra, Content::All);
    assert!(t.screen().cursor.protected);
    assert!(out.contains("\x1b[1\"q"), "protection emit: {out:?}");
}

// "Screen vt with charsets": designation round-trip.
#[test]
fn screen_vt_charsets() {
    // Designate G0 = DEC special graphics (ESC ( 0).
    let t = feed(80, 24, b"\x1b(0");
    let extra = ScreenExtra {
        charsets: true,
        ..Default::default()
    };
    let out = t.screen().format(&Options::vt(), &extra, Content::All);
    assert!(out.contains("\x1b(0"), "charset emit: {out:?}");
    let t2 = feed(80, 24, out.as_bytes());
    assert_eq!(
        t.screen().charset.charsets.get(crate::charsets::Slots::G0),
        t2.screen().charset.charsets.get(crate::charsets::Slots::G0)
    );
}
