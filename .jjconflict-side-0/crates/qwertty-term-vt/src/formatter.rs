//! Serialize screen/terminal state back out as text, VT, or HTML.
//!
//! Port of `src/terminal/formatter.zig` (ghostty commit `2da015cd6`). See
//! `docs/analysis/formatter.md` for the full survey.
//!
//! This is the Rust mirror of the Zig **C-API formatter**
//! (`ghostty_formatter_terminal_*`) that `crates/vt-diff`'s
//! `ReferenceTerminal::raw_text` uses to produce its reference screen dump, so
//! the **plain** output is byte-for-byte the comparison currency.
//!
//! Unlike [`crate::snapshot`], which flattens each cell into an owned
//! `SnapshotCell`, the formatter needs the row-level `wrap`/`wrap_continuation`
//! flags and the per-cell `is_empty`/`has_styling`/`content_tag` distinctions,
//! so it walks the live `PageList` directly (read-only) like
//! [`crate::screen::Screen::dump_string`] does.
//!
//! ## Deferrals (see `docs/analysis/formatter.md`)
//! - `pin_map`/`point_map` byte→pin tracking (perf-heavy render convenience).
//! - Rectangle selection and cross-page x-offset subsets (`Selection.zig`).
//! - HTML `<a>` hyperlink emission (needs `Page::lookup_hyperlink` read-back).

use crate::charsets::{Charset, Slots};
use crate::color::{Palette, Rgb};
use crate::modes::Mode;
use crate::page::style::{Color, Style, Underline};
use crate::page::{Cell, ContentTag, Wide};
use crate::pagelist::Direction;
use crate::point::{Point, Tag};
use crate::screen::Screen;
use crate::terminal::Terminal;

/// The output format. Port of `formatter.zig` `Format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Plain text. Newlines are `\n`.
    Plain,
    /// VT sequences preserving colors/styles. Newlines are `\r\n`.
    Vt,
    /// HTML with inline styles. Newlines are `\n`.
    Html,
}

impl Format {
    /// True if the format emits styled output (not plaintext). Port of
    /// `formatStyled`.
    fn styled(self) -> bool {
        match self {
            Format::Plain => false,
            Format::Vt | Format::Html => true,
        }
    }

    /// The newline sequence used to separate rows in this format.
    fn newline(self) -> &'static str {
        match self {
            // Plain uses `\n`; VT uses `\r\n` so a raw pty returns to col 0;
            // HTML uses `\n`.
            Format::Plain | Format::Html => "\n",
            Format::Vt => "\r\n",
        }
    }
}

/// A codepoint-range replacement. Port of `formatter.zig` `CodepointMap`.
#[derive(Debug, Clone)]
pub struct CodepointMap {
    /// Inclusive `[low, high]` codepoint range to replace.
    pub range: (u32, u32),
    /// The replacement.
    pub replacement: Replacement,
}

/// A `CodepointMap` replacement value.
#[derive(Debug, Clone)]
pub enum Replacement {
    /// Replace with a single codepoint.
    Codepoint(char),
    /// Replace with a UTF-8 string.
    Str(String),
}

/// Common encoding options. Port of `formatter.zig` `Options`.
///
/// Note the port defaults `trim` to true to match the option `Options.plain`
/// preset (and the C API reference dump path).
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The format to emit.
    pub emit: FormatOpt,
    /// Whether to unwrap soft-wrapped lines.
    pub unwrap: bool,
    /// Trim trailing whitespace on rows that have other text; trailing blank
    /// rows are always trimmed.
    pub trim: bool,
    /// Ordered replacement ranges; the **last** matching range wins.
    pub codepoint_map: Vec<CodepointMap>,
    /// Screen background color (VT: OSC 11; HTML: wrapper `background-color`).
    pub background: Option<Rgb>,
    /// Screen foreground color (VT: OSC 10; HTML: wrapper `color`).
    pub foreground: Option<Rgb>,
    /// If set, styled formats emit palette colors as concrete RGB.
    pub palette: Option<Palette>,
}

/// Wrapper so [`Options`] can derive `Default` with `emit = Plain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOpt(pub Format);

impl Default for FormatOpt {
    fn default() -> Self {
        FormatOpt(Format::Plain)
    }
}

impl Options {
    /// `Options.plain` preset: plain text, trim on.
    pub fn plain() -> Self {
        Options {
            emit: FormatOpt(Format::Plain),
            trim: true,
            ..Default::default()
        }
    }

    /// `Options.vt` preset.
    pub fn vt() -> Self {
        Options {
            emit: FormatOpt(Format::Vt),
            trim: true,
            ..Default::default()
        }
    }

    /// `Options.html` preset.
    pub fn html() -> Self {
        Options {
            emit: FormatOpt(Format::Html),
            trim: true,
            ..Default::default()
        }
    }

    fn format(&self) -> Format {
        self.emit.0
    }
}

/// Screen-level extra state to re-emit (VT only). Port of
/// `ScreenFormatter.Extra`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenExtra {
    /// Emit cursor position via CUP.
    pub cursor: bool,
    /// Emit the cursor's active SGR style.
    pub style: bool,
    /// Emit the active OSC8 hyperlink.
    pub hyperlink: bool,
    /// Emit DECSCA protection mode.
    pub protection: bool,
    /// Emit kitty-keyboard flags.
    pub kitty_keyboard: bool,
    /// Emit charset designations/invocations.
    pub charsets: bool,
}

impl ScreenExtra {
    /// `Extra.none`.
    pub fn none() -> Self {
        Self::default()
    }

    /// `Extra.styles`: style + hyperlink only.
    pub fn styles() -> Self {
        ScreenExtra {
            style: true,
            hyperlink: true,
            ..Default::default()
        }
    }

    /// `Extra.all`.
    pub fn all() -> Self {
        ScreenExtra {
            cursor: true,
            style: true,
            hyperlink: true,
            protection: true,
            kitty_keyboard: true,
            charsets: true,
        }
    }

    fn is_set(self) -> bool {
        self.cursor
            || self.style
            || self.hyperlink
            || self.protection
            || self.kitty_keyboard
            || self.charsets
    }
}

/// Terminal-level extra state to re-emit (VT only). Port of
/// `TerminalFormatter.Extra`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalExtra {
    /// Emit the palette via OSC 4.
    pub palette: bool,
    /// Emit modes that differ from defaults.
    pub modes: bool,
    /// Emit scrolling region via DECSTBM/DECSLRM.
    pub scrolling_region: bool,
    /// Emit tabstops via HTS.
    pub tabstops: bool,
    /// Emit pwd via OSC 7.
    pub pwd: bool,
    /// Emit keyboard modes (modifyOtherKeys).
    pub keyboard: bool,
    /// The screen-level extras.
    pub screen: ScreenExtra,
}

impl TerminalExtra {
    /// `Extra.none`.
    pub fn none() -> Self {
        Self::default()
    }

    /// `Extra.styles`: palette + screen styles.
    pub fn styles() -> Self {
        TerminalExtra {
            palette: true,
            screen: ScreenExtra::styles(),
            ..Default::default()
        }
    }

    /// `Extra.all`.
    pub fn all() -> Self {
        TerminalExtra {
            palette: true,
            modes: true,
            scrolling_region: true,
            tabstops: true,
            pwd: true,
            keyboard: true,
            screen: ScreenExtra::all(),
        }
    }
}

/// What content to emit. Port of `ScreenFormatter.Content`.
#[derive(Debug, Clone, Copy, Default)]
pub enum Content {
    /// The whole screen (scrollback + active). Port of `.selection = null`.
    #[default]
    All,
    /// Only rows/cols in the inclusive `[tl, br]` point range (active-area
    /// coordinates). A simplified port of `.selection` (rectangle deferred to
    /// the selection chunk).
    Range { tl: Point, br: Point },
    /// No content, only extra state. Port of `.none`.
    None,
}

// ===========================================================================
// Core renderer (port of PageFormatter.formatWithState)
// ===========================================================================

/// Running blank accounting carried across page boundaries. Port of
/// `PageFormatter.TrailingState`.
#[derive(Debug, Clone, Copy, Default)]
struct TrailingState {
    rows: usize,
    cells: usize,
}

/// Render a point range of a screen's pagelist into `out`. This is the core
/// `PageFormatter.formatWithState` loop, but iterating rows across the whole
/// range at once (the Rust `RowIterator` already spans pages, so the
/// `PageListFormatter`/`PageFormatter` split collapses into one loop with the
/// same accounting).
fn render_range(screen: &Screen, opts: &Options, tl: Point, br: Option<Point>, out: &mut String) {
    let emit = opts.format();

    // Header: VT OSC10/OSC11, HTML wrapper div. Plain: nothing.
    match emit {
        Format::Plain => {}
        Format::Vt => {
            if let Some(fg) = opts.foreground {
                out.push_str(&format!(
                    "\x1b]10;rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    fg.r, fg.g, fg.b
                ));
            }
            if let Some(bg) = opts.background {
                out.push_str(&format!(
                    "\x1b]11;rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    bg.r, bg.g, bg.b
                ));
            }
        }
        Format::Html => {
            out.push_str("<div style=\"font-family: monospace; white-space: pre;");
            if let Some(bg) = opts.background {
                out.push_str(&format!(
                    "background-color: #{:02x}{:02x}{:02x};",
                    bg.r, bg.g, bg.b
                ));
            }
            if let Some(fg) = opts.foreground {
                out.push_str(&format!("color: #{:02x}{:02x}{:02x};", fg.r, fg.g, fg.b));
            }
            out.push_str("\">");
        }
    }

    let mut state = TrailingState::default();

    // Walk every row in [tl, br].
    let mut it = screen.pages.row_iterator(Direction::RightDown, tl, br);
    // SAFETY: the iterator yields valid pins into live pages for the lifetime
    // of `&screen`; we never retain a pin past its use. Mirrors
    // `Screen::dump_string`.
    unsafe {
        let mut style: Style = Style::default();
        let mut blank_rows = state.rows;
        let mut blank_cells = state.cells;

        while let Some(pin) = it.next() {
            let (row, _) = pin.row_and_cell();
            let page = screen.pages.node_data(pin.node);
            let cols = page.size.cols as usize;
            let cells_ptr = page.get_cells(row).cast::<Cell>();
            let cells: &[Cell] = std::slice::from_raw_parts(cells_ptr, cols);

            let wrap = (*row).wrap();
            let wrap_cont = (*row).wrap_continuation();

            // Blank-row accumulation. If this row has no text, defer it.
            if !Cell::has_text_any(cells) {
                blank_rows += 1;
                continue;
            }

            // Flush deferred blank rows as newlines. Reset a non-default style
            // first so bg colors don't bleed across the newline.
            if blank_rows > 0 {
                if !style.is_default() {
                    format_style_close(emit, out);
                    style = Style::default();
                }
                for _ in 0..blank_rows {
                    out.push_str(emit.newline());
                }
                blank_rows = 0;
            }

            // Newline accounting after this row unless we unwrap a wrapped row.
            if !wrap || !opts.unwrap {
                blank_rows += 1;
            }
            // Reset blank-cell run unless we continue a wrap.
            if !wrap_cont || !opts.unwrap {
                blank_cells = 0;
            }

            // Per-cell.
            for cell in cells.iter() {
                match cell.wide() {
                    Wide::Narrow | Wide::Wide => {}
                    Wide::SpacerHead | Wide::SpacerTail => continue,
                }

                // Is this cell blank (deferred)? A styled cell that is empty
                // but carries styling is never blank; otherwise a cell with no
                // text — or a trailing space under `trim` — is blank.
                let is_blank = !(emit.styled() && (!cell.is_empty() || cell.has_styling()))
                    && (!cell.has_text() || (cell.codepoint() == u32::from(' ') && opts.trim));
                if is_blank {
                    blank_cells += 1;
                    continue;
                }

                // Flush deferred blank cells as spaces.
                if blank_cells > 0 {
                    for _ in 0..blank_cells {
                        out.push(' ');
                    }
                    blank_cells = 0;
                }

                // Style minimization (styled formats only).
                if emit.styled() {
                    let cell_style = cell_style(page, cell);
                    if cell_style != style {
                        if !style.is_default() {
                            match emit {
                                Format::Html => format_style_close(emit, out),
                                // VT only closes when switching to default; any
                                // non-default VT style begins with its own reset.
                                Format::Vt => {
                                    if cell_style.is_default() {
                                        format_style_close(emit, out);
                                    }
                                }
                                Format::Plain => unreachable!(),
                            }
                        }
                        style = cell_style;
                        if !cell_style.is_default() {
                            format_style_open(emit, &style, opts.palette.as_ref(), out);
                        }
                    }
                }

                // HTML hyperlinks are deferred (need Page::lookup_hyperlink).

                match cell.content_tag() {
                    ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
                        write_cell(page, cell, cell.content_tag(), emit, opts, out);
                    }
                    // Bg-color-only cells: a space (with the bg style already
                    // opened above via cell_style).
                    ContentTag::BgColorPalette | ContentTag::BgColorRgb => {
                        out.push(' ');
                    }
                }
            }
        }

        // Trailers.
        if !style.is_default() {
            format_style_close(emit, out);
        }
        state.rows = blank_rows;
        state.cells = blank_cells;
    }

    if emit == Format::Html {
        out.push_str("</div>");
    }
    // `state` is intentionally unused after this point (single-range render).
    let _ = state;
}

/// Resolve a cell's [`Style`], synthesizing a bg-only style for bg-color cells.
/// Port of `PageFormatter.cellStyle`.
///
/// # Safety
/// `cell` must belong to `page` and its style id must be live.
unsafe fn cell_style(page: &crate::page::Page, cell: &Cell) -> Style {
    match cell.content_tag() {
        ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
            if !cell.has_styling() {
                Style::default()
            } else {
                // SAFETY: a non-default style id on a live cell is valid.
                unsafe { *page.style_by_id(cell.style_id()) }
            }
        }
        ContentTag::BgColorPalette => Style {
            bg_color: Color::Palette(cell.color_palette()),
            ..Default::default()
        },
        ContentTag::BgColorRgb => {
            let (r, g, b) = cell.color_rgb();
            Style {
                bg_color: Color::Rgb(Rgb { r, g, b }),
                ..Default::default()
            }
        }
    }
}

/// Write a cell's codepoint(s) with replacement + escaping. Port of
/// `PageFormatter.writeCell`.
///
/// # Safety
/// `cell` must belong to `page`.
unsafe fn write_cell(
    page: &crate::page::Page,
    cell: &Cell,
    tag: ContentTag,
    emit: Format,
    opts: &Options,
    out: &mut String,
) {
    if !cell.has_text() {
        out.push(' ');
        return;
    }
    write_codepoint_with_replacement(cell.codepoint(), emit, opts, out);
    if tag == ContentTag::CodepointGrapheme {
        // SAFETY: caller guarantees cell belongs to page.
        if let Some(slice) = unsafe { page.lookup_grapheme(cell as *const Cell as *mut Cell) } {
            // SAFETY: slice valid for the page lifetime.
            for &cp in unsafe { &*slice } {
                write_codepoint_with_replacement(cp, emit, opts, out);
            }
        }
    }
}

/// Port of `PageFormatter.writeCodepointWithReplacement`. Last matching range
/// wins.
fn write_codepoint_with_replacement(cp: u32, emit: Format, opts: &Options, out: &mut String) {
    for entry in opts.codepoint_map.iter().rev() {
        if entry.range.0 <= cp && cp <= entry.range.1 {
            match &entry.replacement {
                Replacement::Codepoint(c) => write_codepoint(u32::from(*c), emit, out),
                Replacement::Str(s) => {
                    for c in s.chars() {
                        write_codepoint(u32::from(c), emit, out);
                    }
                }
            }
            return;
        }
    }
    write_codepoint(cp, emit, out);
}

/// Port of `PageFormatter.writeCodepoint`.
fn write_codepoint(cp: u32, emit: Format, out: &mut String) {
    let Some(c) = char::from_u32(cp) else {
        return;
    };
    match emit {
        Format::Plain | Format::Vt => out.push(c),
        Format::Html => match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => {
                if cp < 0x80 {
                    out.push(c);
                } else {
                    out.push_str(&format!("&#{cp};"));
                }
            }
        },
    }
}

/// Port of `PageFormatter.formatStyleClose`.
fn format_style_close(emit: Format, out: &mut String) {
    match emit {
        Format::Plain => {}
        Format::Vt => out.push_str("\x1b[0m"),
        Format::Html => out.push_str("</div>"),
    }
}

/// Port of `PageFormatter.formatStyleOpen`.
fn format_style_open(emit: Format, style: &Style, palette: Option<&Palette>, out: &mut String) {
    match emit {
        Format::Plain => unreachable!(),
        Format::Vt => format_style_vt(style, palette, out),
        Format::Html => {
            out.push_str("<div style=\"display: inline;");
            format_style_html(style, palette, out);
            out.push_str("\">");
        }
    }
}

/// Port of `style.zig` `Style.VTFormatter`. Always leads with `\x1b[0m`; one
/// separate SGR per attribute (deliberately not combined).
fn format_style_vt(style: &Style, palette: Option<&Palette>, out: &mut String) {
    out.push_str("\x1b[0m");
    let f = &style.flags;
    if f.bold {
        out.push_str("\x1b[1m");
    }
    if f.faint {
        out.push_str("\x1b[2m");
    }
    if f.italic {
        out.push_str("\x1b[3m");
    }
    if f.blink {
        out.push_str("\x1b[5m");
    }
    if f.inverse {
        out.push_str("\x1b[7m");
    }
    if f.invisible {
        out.push_str("\x1b[8m");
    }
    if f.strikethrough {
        out.push_str("\x1b[9m");
    }
    if f.overline {
        out.push_str("\x1b[53m");
    }
    match f.underline {
        Underline::None => {}
        Underline::Single => out.push_str("\x1b[4m"),
        Underline::Double => out.push_str("\x1b[4:2m"),
        Underline::Curly => out.push_str("\x1b[4:3m"),
        Underline::Dotted => out.push_str("\x1b[4:4m"),
        Underline::Dashed => out.push_str("\x1b[4:5m"),
    }
    format_color_vt(38, style.fg_color, palette, out);
    format_color_vt(48, style.bg_color, palette, out);
    format_color_vt(58, style.underline_color, palette, out);
}

fn format_color_vt(prefix: u8, color: Color, palette: Option<&Palette>, out: &mut String) {
    match color {
        Color::None => {}
        Color::Palette(idx) => {
            if let Some(p) = palette {
                let rgb = p[idx as usize];
                out.push_str(&format!("\x1b[{prefix};2;{};{};{}m", rgb.r, rgb.g, rgb.b));
            } else {
                out.push_str(&format!("\x1b[{prefix};5;{idx}m"));
            }
        }
        Color::Rgb(rgb) => {
            out.push_str(&format!("\x1b[{prefix};2;{};{};{}m", rgb.r, rgb.g, rgb.b));
        }
    }
}

/// Port of `style.zig` `Style.HtmlFormatter`.
fn format_style_html(style: &Style, palette: Option<&Palette>, out: &mut String) {
    format_color_html("color", style.fg_color, palette, out);
    format_color_html("background-color", style.bg_color, palette, out);
    format_color_html("text-decoration-color", style.underline_color, palette, out);

    let f = &style.flags;
    let has_line = f.underline != Underline::None || f.strikethrough || f.overline || f.blink;
    if has_line {
        out.push_str("text-decoration-line:");
        if f.underline != Underline::None {
            out.push_str(" underline");
        }
        if f.strikethrough {
            out.push_str(" line-through");
        }
        if f.overline {
            out.push_str(" overline");
        }
        if f.blink {
            out.push_str(" blink");
        }
        out.push(';');
    }

    match f.underline {
        Underline::None => {}
        Underline::Single => out.push_str("text-decoration-style: solid;"),
        Underline::Double => out.push_str("text-decoration-style: double;"),
        Underline::Curly => out.push_str("text-decoration-style: wavy;"),
        Underline::Dotted => out.push_str("text-decoration-style: dotted;"),
        Underline::Dashed => out.push_str("text-decoration-style: dashed;"),
    }

    if f.bold {
        out.push_str("font-weight: bold;");
    }
    if f.italic {
        out.push_str("font-style: italic;");
    }
    if f.faint {
        out.push_str("opacity: 0.5;");
    }
    if f.invisible {
        out.push_str("visibility: hidden;");
    }
    if f.inverse {
        out.push_str("filter: invert(100%);");
    }
}

fn format_color_html(property: &str, color: Color, palette: Option<&Palette>, out: &mut String) {
    match color {
        Color::None => {}
        Color::Palette(idx) => {
            if let Some(p) = palette {
                let rgb = p[idx as usize];
                out.push_str(&format!(
                    "{property}: rgb({}, {}, {});",
                    rgb.r, rgb.g, rgb.b
                ));
            } else {
                out.push_str(&format!("{property}: var(--vt-palette-{idx});"));
            }
        }
        Color::Rgb(rgb) => {
            out.push_str(&format!(
                "{property}: rgb({}, {}, {});",
                rgb.r, rgb.g, rgb.b
            ));
        }
    }
}

// ===========================================================================
// Screen / Terminal entry points
// ===========================================================================

impl Screen {
    /// Format this screen's content. Port of `ScreenFormatter.format`
    /// (content + screen-level `Extra`).
    ///
    /// `content` selects whole-screen / range / none. Extra state is emitted
    /// only for the VT format, after content.
    pub fn format(&self, opts: &Options, extra: &ScreenExtra, content: Content) -> String {
        let mut out = String::new();
        self.format_into(opts, extra, content, &mut out);
        out
    }

    fn format_into(&self, opts: &Options, extra: &ScreenExtra, content: Content, out: &mut String) {
        match content {
            Content::None => {}
            Content::All => {
                // Whole `.screen` space (scrollback + active), matching the C
                // API dump; `None` bottom-right lets the row iterator compute
                // it, exactly like `Screen::dump_string`.
                let tl = Point::new(Tag::Screen, Default::default());
                render_range(self, opts, tl, None, out);
            }
            Content::Range { tl, br } => {
                render_range(self, opts, tl, Some(br), out);
            }
        }

        // Extra state after content, VT only.
        match opts.format() {
            Format::Plain | Format::Html => return,
            Format::Vt => {
                if !extra.is_set() {
                    return;
                }
            }
        }

        if extra.style {
            format_style_vt(&self.cursor.style, opts.palette.as_ref(), out);
        }

        if extra.hyperlink
            && let Some(link) = self.cursor.hyperlink.as_ref()
        {
            emit_hyperlink(link, out);
        }

        if extra.protection && self.cursor.protected {
            out.push_str("\x1b[1\"q");
        }

        if extra.kitty_keyboard {
            let flags = self.kitty_keyboard.current();
            if flags.int() != 0 {
                out.push_str(&format!("\x1b[={};1u", flags.int()));
            }
        }

        if extra.charsets {
            emit_charsets(&self.charset, out);
        }

        if extra.cursor {
            out.push_str(&format!(
                "\x1b[{};{}H",
                self.cursor.y + 1,
                self.cursor.x + 1
            ));
        }
    }
}

/// Emit the active OSC8 hyperlink. Port of the `extra.hyperlink` branch.
fn emit_hyperlink(link: &crate::screen::hyperlink::Hyperlink, out: &mut String) {
    let uri = String::from_utf8_lossy(&link.uri);
    match &link.id {
        crate::screen::hyperlink::HyperlinkId::Explicit(id) => {
            let id = String::from_utf8_lossy(id);
            out.push_str(&format!("\x1b]8;id={id};{uri}\x1b\\"));
        }
        crate::screen::hyperlink::HyperlinkId::Implicit(_) => {
            out.push_str(&format!("\x1b]8;;{uri}\x1b\\"));
        }
    }
}

/// Emit charset designations + GL/GR invocations. Port of the `extra.charsets`
/// branch.
fn emit_charsets(charset: &crate::charsets::CharsetState, out: &mut String) {
    for slot in [Slots::G0, Slots::G1, Slots::G2, Slots::G3] {
        let cs = charset.charsets.get(slot);
        if cs != Charset::Utf8 {
            let intermediate = match slot {
                Slots::G0 => '(',
                Slots::G1 => ')',
                Slots::G2 => '*',
                Slots::G3 => '+',
            };
            let final_ = match cs {
                Charset::Ascii => 'B',
                Charset::British => 'A',
                Charset::DecSpecial => '0',
                Charset::Utf8 => continue,
            };
            out.push('\x1b');
            out.push(intermediate);
            out.push(final_);
        }
    }

    // GL invocation if not G0.
    match charset.gl {
        Slots::G0 => {}
        Slots::G1 => out.push('\x0e'),      // SO
        Slots::G2 => out.push_str("\x1bn"), // LS2
        Slots::G3 => out.push_str("\x1bo"), // LS3
    }

    // GR invocation if not G2.
    match charset.gr {
        Slots::G0 => {}                     // GR can't be G0 (unreachable upstream)
        Slots::G1 => out.push_str("\x1b~"), // LS1R
        Slots::G2 => {}
        Slots::G3 => out.push_str("\x1b|"), // LS3R
    }
}

impl Terminal {
    /// Format the active screen. Port of `TerminalFormatter.format` (whole
    /// screen, styles extra by default).
    pub fn format(&self, opts: &Options, extra: &TerminalExtra) -> String {
        self.format_content(opts, extra, Content::All)
    }

    /// Format a point range (active-area coords) of the active screen. A
    /// simplified port of the `content = selection` path.
    pub fn format_content(
        &self,
        opts: &Options,
        extra: &TerminalExtra,
        content: Content,
    ) -> String {
        let mut out = String::new();
        let emit = opts.format();
        let screen = self.screen();

        // Terminal extras before content (VT only).
        if emit == Format::Vt {
            if extra.palette {
                for (i, rgb) in self.colors.palette.current.iter().enumerate() {
                    out.push_str(&format!(
                        "\x1b]4;{};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                        i, rgb.r, rgb.g, rgb.b
                    ));
                }
            }
            if extra.modes {
                for &mode in Mode::ALL {
                    let current = self.modes.get(mode);
                    if current != self.modes.default_value(mode) {
                        let tag = crate::modes::ModeTag::from_mode(mode);
                        let prefix = if tag.ansi { "" } else { "?" };
                        let suffix = if current { "h" } else { "l" };
                        out.push_str(&format!("\x1b[{prefix}{}{suffix}", tag.value));
                    }
                }
            }
        } else if emit == Format::Html && extra.palette {
            out.push_str("<style>:root{");
            for (i, rgb) in self.colors.palette.current.iter().enumerate() {
                out.push_str(&format!(
                    "--vt-palette-{}: #{:02x}{:02x}{:02x};",
                    i, rgb.r, rgb.g, rgb.b
                ));
            }
            out.push_str("}</style>");
        }

        // Screen content + screen-level extras.
        screen.format_into(opts, &extra.screen, content, &mut out);

        // Terminal extras after content (VT only).
        if emit == Format::Vt {
            if extra.scrolling_region {
                let region = &self.scrolling_region;
                if region.top != 0 || region.bottom != self.rows - 1 {
                    out.push_str(&format!("\x1b[{};{}r", region.top + 1, region.bottom + 1));
                }
                if region.left != 0 || region.right != self.cols - 1 {
                    out.push_str(&format!("\x1b[{};{}s", region.left + 1, region.right + 1));
                }
            }
            if extra.tabstops {
                out.push_str("\x1b[3g");
                for col in 0..self.cols as usize {
                    if self.tabstops.get(col) {
                        out.push_str(&format!("\x1b[{}G", col + 1));
                        out.push_str("\x1bH");
                    }
                }
            }
            if extra.keyboard && self.flags.modify_other_keys_2 {
                out.push_str("\x1b[>4;2m");
            }
            if extra.pwd
                && !self.pwd.is_empty()
                && let Ok(pwd) = std::str::from_utf8(&self.pwd)
            {
                out.push_str(&format!("\x1b]7;{pwd}\x1b\\"));
            }
        }

        out
    }
}

#[cfg(test)]
mod tests;
