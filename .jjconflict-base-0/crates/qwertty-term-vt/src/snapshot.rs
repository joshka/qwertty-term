//! Read-only, owned snapshots of terminal state for embedders / frontends.
//!
//! The internal grid is a page-based, pointer-heavy, refcount-interned
//! structure (see [`crate::pagelist`] and [`crate::page`]); none of it is safe
//! or ergonomic to hand to a renderer directly. This module walks the live
//! pages once and copies the *visible, styled* grid into plain `Vec`-backed
//! owned structs a UI layer can render without `unsafe` and without borrowing
//! the terminal.
//!
//! This is additive read-back API: it does not mutate terminal state and it
//! resolves each cell's interned style id into a concrete [`CellStyle`], so
//! callers get colors and attributes, not just text (which is all
//! [`crate::terminal::Terminal::plain_string`] / `dump_string` give).
//!
//! The scrollback model mirrors what a terminal UI wants: [`Snapshot`] holds
//! *every* row (scrollback history + the active area) as a flat list, plus the
//! index where the active area begins. A frontend keeps its own "scrollback
//! offset from the bottom" and slices a window of `rows` out of `all_rows`
//! without ever mutating the engine's viewport.

use crate::color::{DEFAULT as DEFAULT_PALETTE, Palette, Rgb};
use crate::page::{ContentTag, Wide};
use crate::pagelist::Direction;
use crate::point::{Point, Tag};
use crate::screen::Screen;
use crate::screen::cursor::CursorStyle;
use crate::terminal::{ScreenKey, Terminal};

/// A source-tracked color for a snapshot cell. Mirrors the engine's
/// [`crate::page::style::Color`] but is a plain owned value with no lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotColor {
    /// No explicit color; the renderer should use its default fg/bg.
    #[default]
    Default,
    /// A palette index: `0..=15` are the ANSI/bright colors, `16..=255` the
    /// 256-color cube + grayscale ramp.
    Palette(u8),
    /// A direct 24-bit RGB color.
    Rgb { r: u8, g: u8, b: u8 },
}

/// The underline style of a snapshot cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotUnderline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// The visual style of a single snapshot cell (colors + attributes), resolved
/// from the engine's interned style set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg: SnapshotColor,
    pub bg: SnapshotColor,
    pub underline_color: SnapshotColor,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: SnapshotUnderline,
}

/// Whether a cell is the lead or the trailing spacer of a wide glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellWidth {
    /// A normal single-width cell.
    #[default]
    Narrow,
    /// The lead cell of a double-width glyph.
    Wide,
    /// The trailing spacer cell after a wide glyph (renders nothing).
    Spacer,
}

/// One rendered grid cell: its character(s), width, and resolved style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCell {
    /// The primary character. `' '` for an empty cell. For a spacer this is
    /// `' '` and [`SnapshotCell::is_spacer`] is true.
    pub ch: char,
    /// Any combining/grapheme continuation codepoints after `ch` (usually
    /// empty).
    pub combining: Vec<char>,
    pub width: CellWidth,
    pub style: CellStyle,
    /// The owned identity of this cell's OSC8 hyperlink, if any. Cells sharing
    /// one hyperlink compare equal here, so a renderer can find every cell of a
    /// hovered link (R7). `None` for cells with no hyperlink.
    pub link: Option<crate::page::hyperlink::LinkKey>,
}

impl SnapshotCell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            combining: Vec::new(),
            width: CellWidth::Narrow,
            style: CellStyle::default(),
            link: None,
        }
    }

    /// True if this is the trailing spacer of a wide glyph (renderers skip it).
    pub fn is_spacer(&self) -> bool {
        matches!(self.width, CellWidth::Spacer)
    }

    /// True if this cell is the lead of a double-width glyph.
    pub fn is_wide(&self) -> bool {
        matches!(self.width, CellWidth::Wide)
    }
}

/// One row of snapshot cells (always exactly `cols` entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    pub cells: Vec<SnapshotCell>,
}

impl SnapshotRow {
    /// An all-blank row of `cols` cells (used to pad a window that's shorter
    /// than the requested row count, e.g. just after startup).
    fn blank(cols: usize) -> Self {
        Self {
            cells: vec![SnapshotCell::blank(); cols],
        }
    }
}

/// The cursor position and appearance at snapshot time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCursor {
    /// Column within the active area.
    pub col: usize,
    /// Row within the active area (0 = top of active).
    pub row: usize,
    pub style: CursorStyle,
    pub visible: bool,
    /// Whether the cursor should blink (DEC private mode 12,
    /// `CursorBlinking`). The renderer gates its own blink phase on this via
    /// `FrameOptions.cursor_blink_visible`; when `false` the cursor is drawn
    /// steady. Sourced from terminal mode state (screen-level snapshots leave
    /// it `false`; the `Terminal` wrappers fill in the real mode).
    pub blinking: bool,
}

/// The global (whole-screen) dirty signals a renderer needs to decide whether
/// a frame must be *fully* rebuilt rather than just repainting the per-row
/// dirty rows. Mirrors the union of upstream's full-rebuild triggers that are
/// *intrinsic to the current terminal state* — the `Terminal.Dirty` and
/// `Screen.Dirty` packed-struct bits that `src/terminal/render.zig`'s `update`
/// reads (`t.flags.dirty` and `t.screens.active.dirty`) before forcing
/// `redraw`. The cross-frame triggers (screen-key switch, viewport-pin move,
/// dimension change) are *not* here: those are comparisons against the
/// previous frame, so they're carried as raw values ([`SnapshotWindow`]'s
/// `screen_key`, `window_top`, `cols`/`rows`) for the renderer — which persists
/// across frames, the way upstream's `RenderState` does — to compare itself.
///
/// [`SnapshotWindow::global_dirty_forces_full`] collapses these to the single
/// bool the renderer actually branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapshotDirty {
    /// Palette changed (OSC 4/104). Port of `Terminal.Dirty.palette`.
    pub palette: bool,
    /// Reverse-colors (DECSCNM) toggled. Port of `Terminal.Dirty.reverse_colors`.
    pub reverse_colors: bool,
    /// Screen clear of some kind (erase display, screen change, RIS). Port of
    /// `Terminal.Dirty.clear`.
    pub clear: bool,
    /// Pre-edit modified. Port of `Terminal.Dirty.preedit`.
    pub preedit: bool,
    /// Glyph-protocol registrations changed. Port of
    /// `Terminal.Dirty.glyph_glossary`.
    pub glyph_glossary: bool,
    /// Selection set or unset. Port of `Screen.Dirty.selection`.
    pub selection: bool,
    /// A hovered OSC8 hyperlink dirtied the full screen. Port of
    /// `Screen.Dirty.hyperlink_hover`.
    pub hyperlink_hover: bool,
}

impl SnapshotDirty {
    /// True if any global dirty bit is set, meaning the whole visible frame
    /// must be rebuilt (the renderer can't localize the change to rows).
    /// Mirrors upstream's `if (v > 0) break :redraw true` over the two packed
    /// dirty integers.
    pub fn forces_full(self) -> bool {
        self.palette
            || self.reverse_colors
            || self.clear
            || self.preedit
            || self.glyph_glossary
            || self.selection
            || self.hyperlink_hover
    }
}

/// A cheap, windowed counterpart to [`Snapshot`]: only the rows needed to
/// render a `rows`-tall viewport are materialized, instead of the whole
/// scrollback history.
///
/// A frontend that snapshots once per rendered frame (rather than once per
/// history row written) should use [`Screen::snapshot_window`] /
/// [`Terminal::snapshot_window`] for that per-frame render path, so cost
/// stays proportional to the *visible* window, not to total scrollback
/// length. The full [`Snapshot`] / `snapshot()` API remains for tests,
/// embedding, and anything that needs random access into scrollback (e.g.
/// extracting selected text that may reach above the current window).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWindow {
    pub cols: usize,
    /// Exactly `rows` rows (blank-padded if the terminal has fewer rows of
    /// history than requested, e.g. just after startup).
    pub window: Vec<SnapshotRow>,
    /// The absolute logical row index (as in a full [`Snapshot`]'s
    /// `all_rows`) of `window[0]`. Add a visible row index to this to
    /// recover the same absolute row numbering a full snapshot would use —
    /// needed to keep selection coordinates stable across frames regardless
    /// of which snapshot variant rendered them.
    pub window_top: usize,
    /// The number of scrollback (history) rows above the active area, same
    /// meaning as [`Snapshot::scrollback_len`].
    pub scrollback_len: usize,
    pub cursor: SnapshotCursor,
    pub palette: Palette,
    pub default_fg: Option<Rgb>,
    pub default_bg: Option<Rgb>,
    /// Per-window-row dirty bits (same length and order as `window`): `true` if
    /// that visible row's underlying page/row dirty bit was set when this
    /// window was captured. A renderer doing incremental redraw repaints only
    /// the rows flagged here (unless a global signal forces a full rebuild —
    /// see [`SnapshotWindow::global_dirty`]).
    ///
    /// Only populated by the *tracking* capture path
    /// ([`Terminal::snapshot_window_tracking`] /
    /// [`Screen::snapshot_window_tracking`]), which also *clears* the consumed
    /// dirty bits — mirroring upstream `render.zig`'s `update`. The plain
    /// [`Terminal::snapshot_window`] leaves this all-`true` (every row dirty)
    /// and does not touch the engine's dirty state, so read-only callers and
    /// the differential corpus are unaffected.
    pub row_dirty: Vec<bool>,
    /// The whole-screen dirty signals that force a full rebuild regardless of
    /// per-row dirtiness (palette/selection/clear/etc). Only meaningful on the
    /// tracking capture path; the plain path reports all-`false` here (its
    /// all-`true` `row_dirty` already forces a full repaint).
    pub global_dirty: SnapshotDirty,
    /// Which screen (primary/alternate) this window was captured from. A
    /// renderer compares this against the previous frame's value to detect a
    /// screen switch (alt-screen enter/exit, DEC 1049), which upstream treats
    /// as a full rebuild (`t.screens.active_key != self.screen`).
    pub screen_key: ScreenKey,
    /// Kitty graphics placements visible in this window, resolved to
    /// window-relative draw data (R6 slice 5). Only populated by
    /// [`Terminal::snapshot_window`] / `_tracking` (which have the pixel
    /// geometry); the screen-level capture leaves these empty.
    pub kitty_placements: Vec<crate::kitty::RenderImagePlacement>,
    /// The decoded images referenced by `kitty_placements` (RGBA + generation).
    pub kitty_images: Vec<crate::kitty::SnapshotKittyImage>,
    /// Ids of all images the terminal still holds, for GPU texture eviction.
    pub kitty_live_ids: Vec<u32>,
}

impl SnapshotWindow {
    /// True if a whole-frame rebuild is required by the intrinsic global dirty
    /// signals (palette, selection, clear, etc). Does *not* cover the
    /// cross-frame triggers (screen switch, viewport move, resize); the
    /// renderer detects those by comparing `screen_key` / `window_top` /
    /// `cols` / `rows` against the frame it last drew.
    pub fn global_dirty_forces_full(&self) -> bool {
        self.global_dirty.forces_full()
    }
}

/// A complete, owned, read-only view of the visible + scrollback grid.
///
/// `all_rows` is ordered oldest-first: scrollback history, then the active
/// area. `active_start` is the index of the first active-area row, so
/// `all_rows[active_start..]` is exactly the live grid and everything before it
/// is scrollback. `cursor.row`/`col` are relative to the active area.
///
/// Cells stay *symbolic*: a [`SnapshotCell`]'s style carries [`SnapshotColor`]
/// values (`Default`/`Palette(u8)`/`Rgb`), not resolved pixels. `palette` /
/// `default_fg` / `default_bg` are the terminal's *current* dynamic color
/// state (as of snapshot time — reflecting any OSC 4/10/11/104/110/111/112
/// mutations) that a renderer resolves `SnapshotColor::Palette`/`Default`
/// through. This keeps a copy of the 256-entry palette per snapshot (cheap:
/// `256 * 3` bytes, `Copy`), rather than resolving colors eagerly per cell, so
/// a renderer that wants to special-case `Default` (e.g. to skip painting a
/// bg-colored rect) still can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    pub all_rows: Vec<SnapshotRow>,
    pub active_start: usize,
    pub cursor: SnapshotCursor,
    /// The current 256-color palette (indices 0-255), including any OSC
    /// 4/104 (+ kitty OSC 21, once wired) modifications. Resolve
    /// [`SnapshotColor::Palette`] through this rather than a fixed table.
    pub palette: Palette,
    /// The dynamic default foreground (OSC 10/110), if the terminal has one
    /// set (either a config default or an OSC override). `None` means the
    /// renderer should fall back to its own default.
    pub default_fg: Option<Rgb>,
    /// The dynamic default background (OSC 11/111), if set. `None` means the
    /// renderer should fall back to its own default.
    pub default_bg: Option<Rgb>,
}

impl Snapshot {
    /// The number of scrollback (history) rows above the active area.
    pub fn scrollback_len(&self) -> usize {
        self.active_start
    }

    /// The visible window of `rows` rows, given a scrollback offset measured in
    /// rows *up from the bottom* (0 = the live active area). The returned slice
    /// is always exactly `self.rows` long as long as `all_rows` has at least
    /// that many rows (which it always does — the active area alone is `rows`).
    pub fn visible_window(&self, scrollback_offset: usize) -> &[SnapshotRow] {
        let total = self.all_rows.len();
        let bottom = total.saturating_sub(scrollback_offset.min(self.active_start));
        let top = bottom.saturating_sub(self.rows);
        &self.all_rows[top..bottom]
    }
}

fn to_color(c: crate::page::style::Color) -> SnapshotColor {
    match c {
        crate::page::style::Color::None => SnapshotColor::Default,
        crate::page::style::Color::Palette(i) => SnapshotColor::Palette(i),
        crate::page::style::Color::Rgb(rgb) => SnapshotColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
    }
}

fn to_underline(u: crate::page::style::Underline) -> SnapshotUnderline {
    use crate::page::style::Underline as U;
    match u {
        U::None => SnapshotUnderline::None,
        U::Single => SnapshotUnderline::Single,
        U::Double => SnapshotUnderline::Double,
        U::Curly => SnapshotUnderline::Curly,
        U::Dotted => SnapshotUnderline::Dotted,
        U::Dashed => SnapshotUnderline::Dashed,
    }
}

fn to_cell_style(s: &crate::page::style::Style) -> CellStyle {
    CellStyle {
        fg: to_color(s.fg_color),
        bg: to_color(s.bg_color),
        underline_color: to_color(s.underline_color),
        bold: s.flags.bold,
        faint: s.flags.faint,
        italic: s.flags.italic,
        blink: s.flags.blink,
        inverse: s.flags.inverse,
        invisible: s.flags.invisible,
        strikethrough: s.flags.strikethrough,
        overline: s.flags.overline,
        underline: to_underline(s.flags.underline),
    }
}

impl Screen {
    /// Build an owned [`Snapshot`] of the whole screen (scrollback + active),
    /// resolving each cell's style. Does not mutate any terminal state.
    pub fn snapshot(&self) -> Snapshot {
        let cols = self.cols() as usize;
        let rows = self.rows() as usize;

        // Walk every screen row (history + active), oldest first.
        let it = self.pages.row_iterator(
            Direction::RightDown,
            Point::new(Tag::Screen, Default::default()),
            None,
        );
        let all_rows = copy_rows(&self.pages, it, cols);

        // The active area is the last `rows` rows.
        let active_start = all_rows.len().saturating_sub(rows);

        Snapshot {
            cols,
            rows,
            all_rows,
            active_start,
            cursor: self.snapshot_cursor(),
            // A bare `Screen` doesn't own the terminal's dynamic color
            // state (see `Terminal::colors`); default to the built-in
            // palette / no dynamic override here. `Terminal::snapshot`
            // (below) overwrites these three fields with the live state.
            palette: DEFAULT_PALETTE,
            default_fg: None,
            default_bg: None,
        }
    }

    /// Build an owned [`SnapshotWindow`] containing only the `rows` rows
    /// needed to render a viewport `scrollback_offset` rows up from the
    /// bottom (0 = the live active area), instead of the whole screen.
    ///
    /// This is the cheap, additive counterpart to [`Screen::snapshot`] meant
    /// for a per-rendered-frame call site: finding the window's top row walks
    /// *backward* from the bottom of the page list (cost proportional to the
    /// window height, hopping pages as needed), never forward from the top of
    /// scrollback, so this stays cheap regardless of total history length.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        let cols = self.cols() as usize;
        let rows = self.rows() as usize;
        let total_rows = self.pages.total_rows();
        let scrollback_len = total_rows.saturating_sub(rows);
        let offset = scrollback_offset.min(scrollback_len);

        // The window is exactly `rows` rows ending `offset` rows up from the
        // bottom of the screen.
        //
        // SAFETY: `get_bottom_right` returns a pin into a live page (or
        // `None` for an empty list); `up` only walks `prev` pointers within
        // the same live page list.
        let bottom_right = self
            .pages
            .get_bottom_right(Tag::Screen)
            .and_then(|p| unsafe { p.up(offset) });

        let window = bottom_right.and_then(|bl_pin| {
            let window_len = rows.min(total_rows.saturating_sub(offset));
            let tl_pin = if window_len == 0 {
                None
            } else {
                // SAFETY: see above; `bl_pin` addresses a live page.
                unsafe { bl_pin.up(window_len - 1) }
            };
            tl_pin.map(|tl_pin| {
                // SAFETY: `tl_pin`/`bl_pin` were both derived above by
                // walking `prev` pointers from the live bottom-right pin, so
                // both address live pages; the iterator is only used for the
                // duration of this call.
                let it = unsafe { tl_pin.row_iterator(Direction::RightDown, Some(bl_pin)) };
                copy_rows(&self.pages, it, cols)
            })
        });
        let mut window = window.unwrap_or_default();

        // Pad at the top if the window is shorter than `rows` (e.g. just
        // after startup, before enough rows have been written) so callers
        // can always assume exactly `rows` entries.
        while window.len() < rows {
            window.insert(0, SnapshotRow::blank(cols));
        }

        let row_dirty = vec![true; window.len()];
        SnapshotWindow {
            cols,
            window_top: total_rows.saturating_sub(offset + rows),
            window,
            scrollback_len,
            cursor: self.snapshot_cursor(),
            palette: DEFAULT_PALETTE,
            default_fg: None,
            default_bg: None,
            // Read-only path: every row is "dirty" (full repaint), no global
            // signals, screen-key filled in by `Terminal::snapshot_window`.
            row_dirty,
            global_dirty: SnapshotDirty::default(),
            screen_key: ScreenKey::Primary,
            // Kitty image data is filled in by `Terminal::snapshot_window`
            // (which has the pixel geometry); the screen level leaves it empty.
            kitty_placements: Vec::new(),
            kitty_images: Vec::new(),
            kitty_live_ids: Vec::new(),
        }
    }

    /// Like [`Screen::snapshot_window`], but reads and *clears* the per-row
    /// (and page) dirty bits for the captured window, returning them in
    /// `row_dirty`. This is the renderer's incremental-redraw capture path: a
    /// row is repainted only if its bit is set. Mirrors upstream
    /// `render.zig`'s `update`, which reads each viewport row's `dirty` flag,
    /// then clears the row and page dirty state as it consumes it.
    ///
    /// The global (whole-screen) dirty signals live on the [`Terminal`] /
    /// [`Screen`] as packed flags; this `Screen`-level method only handles the
    /// per-row/page bits, so `global_dirty`/`screen_key` are left at their
    /// defaults for [`Terminal::snapshot_window_tracking`] to fill.
    pub fn snapshot_window_tracking(&mut self, scrollback_offset: usize) -> SnapshotWindow {
        let cols = self.cols() as usize;
        let rows = self.rows() as usize;
        let total_rows = self.pages.total_rows();
        let scrollback_len = total_rows.saturating_sub(rows);
        let offset = scrollback_offset.min(scrollback_len);

        // Same window bounds as `snapshot_window`, but here we also read the
        // row dirty bit for each row and clear it (plus the owning page's
        // dirty flag) as we go.
        let bottom_right = self
            .pages
            .get_bottom_right(Tag::Screen)
            .and_then(|p| unsafe { p.up(offset) });

        let mut window = Vec::new();
        let mut row_dirty = Vec::new();
        if let Some(bl_pin) = bottom_right {
            let window_len = rows.min(total_rows.saturating_sub(offset));
            if window_len > 0 {
                // SAFETY: `bl_pin` addresses a live page; `up` only walks
                // `prev` pointers within the same live page list.
                if let Some(tl_pin) = unsafe { bl_pin.up(window_len - 1) } {
                    // SAFETY: both pins address live pages; the iterator is
                    // used only for the duration of this call.
                    let mut it = unsafe { tl_pin.row_iterator(Direction::RightDown, Some(bl_pin)) };
                    // SAFETY: the iterator yields valid pins into live pages.
                    unsafe {
                        while let Some(pin) = it.next() {
                            let (row_ptr, _) = pin.row_and_cell();
                            let page = self.pages.node_data(pin.node);
                            let dirty = (*pin.node).data.dirty || (*row_ptr).dirty();
                            row_dirty.push(dirty);
                            // Consume (clear) the row dirty bit as upstream does.
                            (*row_ptr).set_dirty(false);

                            let base = page.get_cells(row_ptr) as *const crate::page::Cell;
                            let mut cells = Vec::with_capacity(cols);
                            for x in 0..cols {
                                let cell = *base.add(x);
                                cells.push(snapshot_cell(page, base.add(x), &cell));
                            }
                            window.push(SnapshotRow { cells });
                        }
                    }
                }
            }
        }

        // Clear every page's dirty flag we may have observed. Upstream clears
        // the page dirty of the last dirty page inline and finalizes any
        // trailing one; we simply clear all pages' dirty bits after consuming
        // them (cheap: page count is tiny), matching `render.zig`'s effect of
        // leaving no page dirty behind.
        self.pages.clear_page_dirty();

        // Pad at the top if the window is shorter than `rows` (parity with
        // `snapshot_window`); padded rows are treated as dirty (they're
        // freshly synthesized, so must be painted).
        while window.len() < rows {
            window.insert(0, SnapshotRow::blank(cols));
            row_dirty.insert(0, true);
        }

        SnapshotWindow {
            cols,
            window_top: total_rows.saturating_sub(offset + rows),
            window,
            scrollback_len,
            cursor: self.snapshot_cursor(),
            palette: DEFAULT_PALETTE,
            default_fg: None,
            default_bg: None,
            row_dirty,
            global_dirty: SnapshotDirty::default(),
            screen_key: ScreenKey::Primary,
            // Kitty image data is filled in by `Terminal::snapshot_window`
            // (which has the pixel geometry); the screen level leaves it empty.
            kitty_placements: Vec::new(),
            kitty_images: Vec::new(),
            kitty_live_ids: Vec::new(),
        }
    }

    fn snapshot_cursor(&self) -> SnapshotCursor {
        SnapshotCursor {
            col: self.cursor.x as usize,
            row: self.cursor.y as usize,
            style: self.cursor.cursor_style,
            visible: true,
            // Screen has no mode state; the `Terminal` wrappers override this
            // from mode 12 (see `Terminal::snapshot*`), matching `visible`.
            blinking: false,
        }
    }
}

/// Copy every row an iterator yields into owned [`SnapshotRow`]s.
///
/// # Safety
/// `it` must only yield pins into `pages`'s live pages.
fn copy_rows(
    pages: &crate::pagelist::PageList,
    mut it: crate::pagelist::RowIterator,
    cols: usize,
) -> Vec<SnapshotRow> {
    let mut rows = Vec::new();
    // SAFETY: the iterator yields valid pins into live pages for the
    // lifetime of `pages`; we never retain a pin past its use.
    unsafe {
        while let Some(pin) = it.next() {
            let (row, _) = pin.row_and_cell();
            let page = pages.node_data(pin.node);
            let base = page.get_cells(row);
            let base = base as *const crate::page::Cell;

            let mut cells = Vec::with_capacity(cols);
            for x in 0..cols {
                let cell = *base.add(x);
                cells.push(snapshot_cell(page, base.add(x), &cell));
            }
            rows.push(SnapshotRow { cells });
        }
    }
    rows
}

/// Extract one owned cell from a live page cell.
///
/// # Safety
/// `cell_ptr` must point at `cell` inside `page`'s live memory.
unsafe fn snapshot_cell(
    page: &crate::page::Page,
    cell_ptr: *const crate::page::Cell,
    cell: &crate::page::Cell,
) -> SnapshotCell {
    let width = match cell.wide() {
        Wide::Narrow => CellWidth::Narrow,
        Wide::Wide => CellWidth::Wide,
        Wide::SpacerTail | Wide::SpacerHead => CellWidth::Spacer,
    };

    // Background-color-only cells carry no text but do carry a bg color in the
    // content bits (used by the erase/bg-fill paths). Surface that as a blank
    // cell with the bg set so the renderer paints it.
    let (ch, combining) = match cell.content_tag() {
        ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
            let cp = cell.codepoint();
            let ch = if cp == 0 {
                ' '
            } else {
                char::from_u32(cp).unwrap_or(' ')
            };
            let mut combining = Vec::new();
            if cell.content_tag() == ContentTag::CodepointGrapheme {
                // SAFETY: cell_ptr addresses this cell in this page.
                if let Some(slice) = unsafe { page.lookup_grapheme(cell_ptr as *mut _) } {
                    // SAFETY: slice valid for the page lifetime.
                    for &g in unsafe { &*slice } {
                        if let Some(c) = char::from_u32(g) {
                            combining.push(c);
                        }
                    }
                }
            }
            (ch, combining)
        }
        ContentTag::BgColorPalette | ContentTag::BgColorRgb => (' ', Vec::new()),
    };

    let mut style = if cell.style_id() != crate::page::style::DEFAULT_ID {
        // SAFETY: a non-default style id on a live cell is valid in this page's
        // style set with ref count > 0.
        let s = unsafe { &*page.style_by_id(cell.style_id()) };
        to_cell_style(s)
    } else {
        CellStyle::default()
    };

    // Fold a bg-color-only cell's color into the style bg.
    match cell.content_tag() {
        ContentTag::BgColorPalette => style.bg = SnapshotColor::Palette(cell.color_palette()),
        ContentTag::BgColorRgb => {
            let (r, g, b) = cell.color_rgb();
            style.bg = SnapshotColor::Rgb { r, g, b };
        }
        _ => {}
    }

    // Carry the cell's hyperlink identity (R7) so a renderer can match every
    // cell of a hovered link. Only cells with the hyperlink bit set have one.
    let link = if cell.hyperlink() {
        // SAFETY: `cell_ptr` addresses this cell in this page (caller contract).
        unsafe { page.hyperlink_key(cell_ptr) }
    } else {
        None
    };

    SnapshotCell {
        ch,
        combining,
        width,
        style,
        link,
    }
}

impl Terminal {
    /// Build an owned [`Snapshot`] of the active screen. Convenience wrapper
    /// over [`Screen::snapshot`] that also reports the real cursor visibility
    /// (DEC mode 25) from terminal mode state and the live dynamic color
    /// state (`self.colors`, mutated by OSC 4/5/10-19/104/105/110-119 via
    /// `TerminalHandler::color_operation`) rather than a fixed default.
    pub fn snapshot(&self) -> Snapshot {
        let mut snap = self.screen().snapshot();
        snap.cursor.visible = self.modes.get(crate::modes::Mode::CursorVisible);
        snap.cursor.blinking = self.modes.get(crate::modes::Mode::CursorBlinking);
        // A blank far-right column can drop out of Tag::Screen's bottom-right if
        // the whole active area is empty; guarantee the row count invariant.
        while snap.all_rows.len() < snap.rows {
            snap.all_rows.push(SnapshotRow {
                cells: vec![SnapshotCell::blank(); snap.cols],
            });
        }
        snap.active_start = snap.all_rows.len().saturating_sub(snap.rows);
        snap.palette = self.colors.palette.current;
        snap.default_fg = self.colors.foreground.get();
        snap.default_bg = self.colors.background.get();
        snap
    }

    /// Build an owned [`SnapshotWindow`] of just the rows needed to render a
    /// viewport `scrollback_offset` rows up from the bottom. Convenience
    /// wrapper over [`Screen::snapshot_window`] that also reports the real
    /// cursor visibility and live dynamic color state, matching
    /// [`Terminal::snapshot`]'s behavior over [`Screen::snapshot`].
    ///
    /// Prefer this over [`Terminal::snapshot`] on a per-rendered-frame call
    /// site (e.g. a UI redraw loop): cost is proportional to `rows`, not to
    /// total scrollback length.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        let (kitty_placements, kitty_images, kitty_live_ids) =
            self.resolve_kitty_window(scrollback_offset);
        let mut snap = self.screen().snapshot_window(scrollback_offset);
        snap.cursor.visible = self.modes.get(crate::modes::Mode::CursorVisible);
        snap.cursor.blinking = self.modes.get(crate::modes::Mode::CursorBlinking);
        snap.palette = self.colors.palette.current;
        snap.default_fg = self.colors.foreground.get();
        snap.default_bg = self.colors.background.get();
        snap.screen_key = self.screens.active_key();
        snap.kitty_placements = kitty_placements;
        snap.kitty_images = kitty_images;
        snap.kitty_live_ids = kitty_live_ids;
        snap
    }

    /// Resolve this frame's kitty image draw data for the window
    /// `scrollback_offset` rows up (R6 slice 5). Uses the terminal's pixel
    /// geometry + the active screen's image storage/pages; returns empty vecs
    /// when kitty storage is disabled or holds no images.
    fn resolve_kitty_window(
        &self,
        scrollback_offset: usize,
    ) -> (
        Vec<crate::kitty::RenderImagePlacement>,
        Vec<crate::kitty::SnapshotKittyImage>,
        Vec<u32>,
    ) {
        let screen = self.screen();
        let geo = crate::kitty::TerminalGeometry {
            cols: self.cols,
            rows: self.rows,
            width_px: self.width_px,
            height_px: self.height_px,
        };
        crate::kitty::resolve_window(&screen.kitty_images, &screen.pages, &geo, scrollback_offset)
    }

    /// Build an owned [`SnapshotWindow`] for the incremental-redraw render
    /// path: like [`Terminal::snapshot_window`], but it reads and **clears**
    /// the consumed dirty state (per-row/page bits plus the global
    /// `Terminal.Dirty`/`Screen.Dirty` flags), reporting which rows were dirty
    /// in `row_dirty` and the global signals in `global_dirty`. This mirrors
    /// upstream `render.zig`'s `update`, which is the single place that
    /// consumes and resets the terminal's renderer dirty state.
    ///
    /// A renderer calls this once per frame it draws (not once per snapshot
    /// inspection) and repaints only the dirty rows, falling back to a full
    /// rebuild when `global_dirty` forces it or when the cross-frame signals
    /// (`screen_key`, `window_top`, `cols`/`rows`) show a screen switch,
    /// viewport move, or resize. Read-only inspection must use the plain
    /// [`Terminal::snapshot_window`], which never mutates dirty state.
    pub fn snapshot_window_tracking(&mut self, scrollback_offset: usize) -> SnapshotWindow {
        // Snapshot the global dirty flags *before* the screen-level capture
        // clears anything (it only touches row/page bits, but read first to be
        // unambiguous).
        let global_dirty = SnapshotDirty {
            palette: self.flags.dirty.palette,
            reverse_colors: self.flags.dirty.reverse_colors,
            clear: self.flags.dirty.clear,
            preedit: self.flags.dirty.preedit,
            glyph_glossary: self.flags.dirty.glyph_glossary,
            selection: self.screen().dirty.selection,
            hyperlink_hover: self.screen().dirty.hyperlink_hover,
        };
        let screen_key = self.screens.active_key();

        let cursor_visible = self.modes.get(crate::modes::Mode::CursorVisible);
        let cursor_blinking = self.modes.get(crate::modes::Mode::CursorBlinking);
        let palette = self.colors.palette.current;
        let default_fg = self.colors.foreground.get();
        let default_bg = self.colors.background.get();
        // Resolve kitty data under the immutable borrow, before taking
        // `screen_mut` for the windowed capture.
        let (kitty_placements, kitty_images, kitty_live_ids) =
            self.resolve_kitty_window(scrollback_offset);

        let mut snap = self
            .screen_mut()
            .snapshot_window_tracking(scrollback_offset);
        snap.cursor.visible = cursor_visible;
        snap.cursor.blinking = cursor_blinking;
        snap.palette = palette;
        snap.default_fg = default_fg;
        snap.default_bg = default_bg;
        snap.global_dirty = global_dirty;
        snap.screen_key = screen_key;
        snap.kitty_placements = kitty_placements;
        snap.kitty_images = kitty_images;
        snap.kitty_live_ids = kitty_live_ids;

        // Consume (clear) the global dirty flags, exactly as upstream's
        // `render.zig` `update` does at its tail (`t.flags.dirty = .{};
        // s.dirty = .{};`).
        self.flags.dirty = crate::terminal::Dirty::default();
        self.screen_mut().dirty = crate::screen::Dirty::default();

        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Stream, TerminalHandler};
    use crate::terminal::{Options, Terminal};

    fn feed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
        let term = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(bytes);
        stream.handler.terminal
    }

    fn row_text(row: &SnapshotRow) -> String {
        let mut s: String = row
            .cells
            .iter()
            .filter(|c| !c.is_spacer())
            .map(|c| c.ch)
            .collect();
        while s.ends_with(' ') {
            s.pop();
        }
        s
    }

    #[test]
    fn plain_text_snapshot() {
        let term = feed(10, 3, b"Hello");
        let snap = term.snapshot();
        assert_eq!(snap.cols, 10);
        assert_eq!(snap.rows, 3);
        assert_eq!(row_text(&snap.all_rows[snap.active_start]), "Hello");
        assert_eq!(snap.cursor.col, 5);
        assert_eq!(snap.cursor.row, 0);
        assert!(snap.cursor.visible);
    }

    #[test]
    fn cursor_blinking_tracks_mode_12() {
        // Default: DEC mode 12 (CursorBlinking) is off → steady cursor.
        let term = feed(10, 3, b"x");
        assert!(!term.snapshot().cursor.blinking, "default: not blinking");
        assert!(!term.snapshot_window(0).cursor.blinking);

        // Enable mode 12 → all three snapshot paths report blinking.
        let mut term = feed(10, 3, b"\x1b[?12h");
        assert!(
            term.snapshot().cursor.blinking,
            "snapshot() carries mode 12"
        );
        assert!(
            term.snapshot_window(0).cursor.blinking,
            "snapshot_window() carries mode 12"
        );
        assert!(
            term.snapshot_window_tracking(0).cursor.blinking,
            "snapshot_window_tracking() carries mode 12"
        );

        // Disable again (12h then 12l) → steady.
        let term = feed(10, 3, b"\x1b[?12h\x1b[?12l");
        assert!(!term.snapshot().cursor.blinking, "mode 12 reset → steady");
    }

    #[test]
    fn snapshot_carries_hyperlink_identity() {
        use crate::page::hyperlink::LinkKey;
        // OSC8 open (implicit id, uri) → "ab" → close → "c".
        let term = feed(10, 3, b"\x1b]8;;http://example.com\x1b\\ab\x1b]8;;\x1b\\c");
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];

        let a = row.cells[0].link.clone();
        let b = row.cells[1].link.clone();
        assert!(a.is_some(), "a linked cell carries a LinkKey");
        assert_eq!(a, b, "the two cells of one hyperlink share identity");
        assert_eq!(row.cells[2].link, None, "the non-link cell has no key");

        // The URI bytes are captured on the key.
        let uri = match a.unwrap() {
            LinkKey::Implicit(_, uri) | LinkKey::Explicit(_, uri) => uri,
        };
        assert_eq!(uri, b"http://example.com");
    }

    #[test]
    fn distinct_hyperlinks_do_not_share_identity() {
        use crate::page::hyperlink::LinkKey;
        // Two separate implicit links to *different* URIs on one row.
        let term = feed(
            12,
            3,
            b"\x1b]8;;http://a.test\x1b\\a\x1b]8;;\x1b\\\x1b]8;;http://b.test\x1b\\b\x1b]8;;\x1b\\",
        );
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];
        let a = row.cells[0].link.clone().expect("cell a linked");
        let b = row.cells[1].link.clone().expect("cell b linked");
        assert_ne!(a, b, "different URIs are different links");
        let uri = |k: LinkKey| match k {
            LinkKey::Implicit(_, u) | LinkKey::Explicit(_, u) => u,
        };
        assert_eq!(uri(a), b"http://a.test");
        assert_eq!(uri(b), b"http://b.test");
    }

    #[test]
    fn sgr_colors_are_resolved() {
        // Red fg (31), blue bg (44), bold (1) on 'A', reset then 'B'.
        let term = feed(10, 2, b"\x1b[1;31;44mA\x1b[0mB");
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];
        assert_eq!(row.cells[0].ch, 'A');
        assert_eq!(row.cells[0].style.fg, SnapshotColor::Palette(1));
        assert_eq!(row.cells[0].style.bg, SnapshotColor::Palette(4));
        assert!(row.cells[0].style.bold);
        assert_eq!(row.cells[1].ch, 'B');
        assert_eq!(row.cells[1].style, CellStyle::default());
    }

    #[test]
    fn rgb_and_256_colors() {
        let term = feed(10, 2, b"\x1b[38;5;196;48;2;1;2;3mX");
        let snap = term.snapshot();
        let cell = &snap.all_rows[snap.active_start].cells[0];
        assert_eq!(cell.style.fg, SnapshotColor::Palette(196));
        assert_eq!(cell.style.bg, SnapshotColor::Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn wide_char_lead_and_spacer() {
        let term = feed(10, 2, "好x".as_bytes());
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];
        assert_eq!(row.cells[0].ch, '好');
        assert!(row.cells[0].is_wide());
        assert!(row.cells[1].is_spacer());
        assert_eq!(row.cells[2].ch, 'x');
    }

    #[test]
    fn cursor_hidden_mode() {
        let term = feed(10, 2, b"\x1b[?25l");
        let snap = term.snapshot();
        assert!(!snap.cursor.visible);
    }

    // DECSCUSR (`CSI Ps SP q`) should stick on the cursor snapshot, not just
    // flip the blink mode. Regression test for the shell-integration bar
    // cursor (`CSI 5 SP q`) rendering as a block.
    #[test]
    fn decscusr_sets_snapshot_cursor_style() {
        use crate::screen::cursor::CursorStyle;

        let term = feed(10, 2, b"\x1b[5 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Bar);

        let term = feed(10, 2, b"\x1b[6 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Bar);

        let term = feed(10, 2, b"\x1b[3 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Underline);

        let term = feed(10, 2, b"\x1b[4 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Underline);

        let term = feed(10, 2, b"\x1b[1 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        let term = feed(10, 2, b"\x1b[2 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        // CSI 0 SP q (default) resets to block.
        let term = feed(10, 2, b"\x1b[5 q\x1b[0 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        // Full reset (RIS) restores the default block style.
        let term = feed(10, 2, b"\x1b[5 q\x1bc");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);
    }

    #[test]
    fn scrollback_rows_precede_active() {
        // 4 cols, 2 rows; write enough to push a row into scrollback.
        let term = feed(4, 2, b"aaaabbbbcccc");
        let snap = term.snapshot();
        assert!(snap.scrollback_len() >= 1);
        // The visible window (offset 0) is the active area.
        let window = snap.visible_window(0);
        assert_eq!(window.len(), 2);
        // Scrolling up by 1 reveals a history row at the top.
        let up = snap.visible_window(1);
        assert_eq!(up.len(), 2);
        assert_ne!(row_text(&up[0]), row_text(&window[0]));
    }

    #[test]
    fn default_snapshot_uses_default_palette_and_no_dynamic_overrides() {
        let term = feed(10, 2, b"");
        let snap = term.snapshot();
        assert_eq!(snap.palette, crate::color::DEFAULT);
        assert_eq!(snap.default_fg, None);
        assert_eq!(snap.default_bg, None);
    }

    #[test]
    fn osc_4_set_color_is_reflected_in_snapshot_palette() {
        // OSC 4: set palette index 1 (normally red) to a custom color.
        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::Rgb::new(0x11, 0x22, 0x33));
        // Untouched entries keep their default value.
        assert_eq!(snap.palette[2], crate::color::DEFAULT[2]);
    }

    #[test]
    fn osc_104_reset_restores_default_palette() {
        // Set index 1, then reset just that index (OSC 104;1) and confirm it
        // reverts; then set again and reset-all (bare OSC 104).
        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b\\\x1b]104;1\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::DEFAULT[1]);

        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b]4;2;#445566\x1b]104\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::DEFAULT[1]);
        assert_eq!(snap.palette[2], crate::color::DEFAULT[2]);
    }

    #[test]
    fn osc_10_11_set_default_fg_bg_and_osc_110_111_reset() {
        // OSC 10 = fg, OSC 11 = bg.
        let term = feed(10, 2, b"\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\");
        let snap = term.snapshot();
        assert_eq!(
            snap.default_fg,
            Some(crate::color::Rgb::new(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            snap.default_bg,
            Some(crate::color::Rgb::new(0x00, 0x11, 0x22))
        );

        // OSC 110/111 reset fg/bg back to unset.
        let term = feed(
            10,
            2,
            b"\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\",
        );
        let snap = term.snapshot();
        assert_eq!(snap.default_fg, None);
        assert_eq!(snap.default_bg, None);
    }

    #[test]
    fn snapshot_window_matches_full_snapshot_active_area() {
        let term = feed(10, 3, b"Hello");
        let full = term.snapshot();
        let window = term.snapshot_window(0);

        assert_eq!(window.cols, full.cols);
        assert_eq!(window.window.len(), full.rows);
        assert_eq!(window.window, full.all_rows[full.active_start..].to_vec());
        assert_eq!(window.cursor, full.cursor);
        assert_eq!(window.scrollback_len, full.scrollback_len());
    }

    #[test]
    fn snapshot_window_matches_full_snapshot_when_scrolled_into_history() {
        // 4 cols, 2 rows; push several rows into scrollback.
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let full = term.snapshot();
        assert!(full.scrollback_len() >= 2);

        for offset in 0..=full.scrollback_len() {
            let window = term.snapshot_window(offset);
            let expected = full.visible_window(offset);
            assert_eq!(
                window.window, expected,
                "window mismatch at offset {offset}"
            );
        }
    }

    #[test]
    fn snapshot_window_top_is_absolute_logical_row() {
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let full = term.snapshot();
        let window = term.snapshot_window(0);

        // window[0] should be the same row as `all_rows[window_top]` in a
        // full snapshot, preserving absolute row numbering for selection.
        assert_eq!(window.window[0], full.all_rows[window.window_top]);
    }

    #[test]
    fn snapshot_window_always_has_exactly_rows_entries() {
        // A fresh terminal's active area always has exactly `rows` rows (the
        // page list is initialized with `rows` rows up front), so the window
        // is always fully populated, never padded.
        let term = feed(10, 5, b"hi");
        let window = term.snapshot_window(0);
        assert_eq!(window.window.len(), 5);
        assert_eq!(row_text(&window.window[0]), "hi");
    }

    #[test]
    fn snapshot_window_clamps_offset_beyond_scrollback() {
        let term = feed(4, 2, b"aaaabbbbcccc");
        let full = term.snapshot();
        let clamped = term.snapshot_window(full.scrollback_len() + 100);
        let unclamped = term.snapshot_window(full.scrollback_len());
        assert_eq!(clamped.window, unclamped.window);
    }

    // ---- tracking capture path ----------------------------------------

    fn stream_of(term: Terminal) -> Stream<TerminalHandler> {
        Stream::new(TerminalHandler::new(term))
    }

    #[test]
    fn tracking_window_content_matches_plain_window() {
        // The tracking path must produce byte-identical cell content to the
        // plain path (it only additionally reports+clears dirty state).
        let term = feed(10, 3, b"Hello\r\nWorld");
        let plain = term.snapshot_window(0);
        let mut term = term;
        let tracking = term.snapshot_window_tracking(0);
        assert_eq!(tracking.window, plain.window);
        assert_eq!(tracking.cursor, plain.cursor);
        assert_eq!(tracking.screen_key, plain.screen_key);
        assert_eq!(tracking.row_dirty.len(), tracking.window.len());
    }

    #[test]
    fn tracking_reports_only_the_touched_row_after_a_clean_capture() {
        // Fresh writes make rows dirty. A first tracking capture consumes+
        // clears them; a follow-up with no writes reports all-clean; a write
        // touching exactly one row reports exactly that row dirty.
        let mut term = feed(10, 4, b"aaa\r\nbbb\r\nccc\r\nddd");

        // First capture: some rows dirty (freshly written).
        let first = term.snapshot_window_tracking(0);
        assert!(first.row_dirty.iter().any(|&d| d), "first capture has dirt");

        // Second capture with no intervening writes: nothing dirty.
        let clean = term.snapshot_window_tracking(0);
        assert!(
            clean.row_dirty.iter().all(|&d| !d),
            "clean capture has no dirty rows, got {:?}",
            clean.row_dirty
        );

        // Move cursor to row 2 (0-based) and overwrite it only.
        let mut stream = stream_of(term);
        stream.feed(b"\x1b[3;1HXYZ"); // CUP row 3 (1-based) => row index 2
        let mut term = stream.handler.terminal;

        let partial = term.snapshot_window_tracking(0);
        // The rewritten row is dirty; the untouched rows above it stay clean.
        // (The cursor's landing/prior row may also be flagged, matching the
        // engine's dirty semantics — we only require that unrelated rows are
        // NOT needlessly repainted.)
        assert!(partial.row_dirty[2], "rewritten row 2 dirty");
        assert!(!partial.row_dirty[0], "row 0 stays clean");
        assert!(!partial.row_dirty[1], "row 1 stays clean");
        // And the touched row has the new content.
        assert_eq!(row_text(&partial.window[2]), "XYZ");
    }

    #[test]
    fn tracking_consumes_global_palette_dirty() {
        // OSC 4 sets Terminal.Dirty.palette; the tracking capture reports it
        // and then clears it so the next capture is clean.
        let mut term = feed(10, 2, b"hi");
        // Drain the initial write dirt.
        let _ = term.snapshot_window_tracking(0);

        let mut stream = stream_of(term);
        stream.feed(b"\x1b]4;1;#112233\x1b\\");
        let mut term = stream.handler.terminal;

        let snap = term.snapshot_window_tracking(0);
        assert!(snap.global_dirty.palette, "palette dirty reported");
        assert!(snap.global_dirty_forces_full());

        // Consumed: next capture clean.
        let next = term.snapshot_window_tracking(0);
        assert!(!next.global_dirty.palette);
        assert!(!next.global_dirty_forces_full());
    }

    #[test]
    fn tracking_reports_selection_dirty() {
        let mut term = feed(10, 3, b"hello world");
        let _ = term.snapshot_window_tracking(0);

        // Set a selection (Screen.Dirty.selection).
        use crate::point::Point;
        use crate::screen::selection::Selection;
        let s = term.screen_mut();
        let start = s.pages.pin(Point::active(0, 0)).unwrap();
        let end = s.pages.pin(Point::active(4, 0)).unwrap();
        s.select(Some(Selection::init(start, end, false)));

        let snap = term.snapshot_window_tracking(0);
        assert!(snap.global_dirty.selection, "selection dirty reported");
        assert!(snap.global_dirty_forces_full());

        let next = term.snapshot_window_tracking(0);
        assert!(!next.global_dirty.selection);
    }

    #[test]
    fn tracking_screen_key_tracks_alt_screen() {
        let mut term = feed(10, 2, b"primary");
        assert_eq!(
            term.snapshot_window_tracking(0).screen_key,
            crate::terminal::ScreenKey::Primary
        );

        let mut stream = stream_of(term);
        stream.feed(b"\x1b[?1049h"); // enter alt screen
        let mut term = stream.handler.terminal;
        assert_eq!(
            term.snapshot_window_tracking(0).screen_key,
            crate::terminal::ScreenKey::Alternate
        );
    }

    #[test]
    fn plain_window_leaves_dirty_untouched() {
        // The read-only path must not clear dirty state (differential corpus
        // and inspection callers depend on this).
        let mut term = feed(10, 2, b"data");
        // Plain capture reports all rows dirty and leaves the bits set...
        let plain = term.snapshot_window(0);
        assert!(plain.row_dirty.iter().all(|&d| d));
        // ...so a subsequent tracking capture still sees them dirty.
        let tracking = term.snapshot_window_tracking(0);
        assert!(tracking.row_dirty.iter().any(|&d| d));
    }
}
