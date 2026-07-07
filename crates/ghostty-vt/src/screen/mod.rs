//! The viewport/cursor layer over `PageList` (port of `src/terminal/Screen.zig`,
//! commit `2da015cd6`).
//!
//! See `docs/analysis/screen.md` for the maintainer-grade map. In short, this
//! layer owns the cursor (and its style/hyperlink caching against the cursor's
//! page), charset state, the kitty keyboard flag stack, semantic-prompt state,
//! dirty render hints, and the resize entry point; it delegates all memory and
//! layout concerns to `PageList`.
//!
//! Scope of this chunk: cursor management, scrolling, clearing/erase, resize
//! wiring, style/hyperlink caching, dirty plumbing, kitty keyboard, semantic
//! prompt. Selection is SCAFFOLD only; SGR/charset tables and OSC parsing land
//! with sibling chunks (see the `TODO(chunk:*)` markers).

pub mod cursor;
pub mod hyperlink;
pub mod kitty_key;
pub mod selection;
pub mod semantic;

use crate::page::size::CellCountInt;
use crate::page::style::{self, Style};
use crate::page::{
    Cell, HyperlinkInsertId, InsertHyperlinkError, Page, SemanticContent,
    SemanticPrompt as RowSemanticPrompt,
};
use crate::pagelist::{
    Direction, IncreaseCapacity, PageList, Pin, Resize as PageResize, ResizeCursor,
    Scroll as PageScroll,
};
use crate::point::{Point, Tag};

use crate::charsets::CharsetState;
use cursor::{Cursor, CursorCopy, SavedCursor};
use hyperlink::{Hyperlink, HyperlinkId};
use kitty_key::FlagStack;
use selection::{Order, Selection};
use semantic::{PromptKind, Redraw, SemanticPrompt};

/// The character protection mode. Port of `ansi.ProtectedMode`. Owned as
/// Terminal-adjacent state but stored on `Screen` because erase paths read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtectedMode {
    #[default]
    Off,
    Iso,
    Dec,
}

/// Renderer-facing dirty flags. Port of `Screen.Dirty`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Dirty {
    /// Set when the selection is set or unset (regardless of change).
    pub selection: bool,
    /// Set when a hovered OSC8 hyperlink dirties the full screen.
    pub hyperlink_hover: bool,
}

/// Options for constructing a [`Screen`]. Port of `Screen.Options`.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub cols: CellCountInt,
    pub rows: CellCountInt,
    /// Max scrollback in bytes; 0 = no scrollback.
    pub max_scrollback: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        }
    }
}

/// Scroll behaviors. Port of `Screen.Scroll` (mirrors `PageList.Scroll`).
#[derive(Debug, Clone, Copy)]
pub enum Scroll {
    Active,
    Top,
    Pin(Pin),
    Row(usize),
    DeltaRow(isize),
    DeltaPrompt(isize),
}

/// Resize options. Port of `Screen.Resize`.
#[derive(Debug, Clone, Copy)]
pub struct Resize {
    pub cols: CellCountInt,
    pub rows: CellCountInt,
    pub reflow: bool,
    pub prompt_redraw: Redraw,
}

/// The viewport/cursor layer. Port of `Screen`.
pub struct Screen {
    /// The paged scrollback list.
    pub pages: PageList,

    /// Special case: no scrollback whatsoever (PageList rounds max_size 0 up).
    pub no_scrollback: bool,

    /// The current cursor.
    pub cursor: Cursor,

    /// The saved cursor (DECSC).
    pub saved_cursor: Option<SavedCursor>,

    /// The selection for this screen (if any). This MUST be a tracked selection,
    /// otherwise it will become invalid. Prefer [`Screen::select`] /
    /// [`Screen::clear_selection`] over mutating this directly. Port of
    /// `Screen.selection`.
    pub selection: Option<Selection>,

    /// The charset state. See [`crate::charsets::CharsetState`].
    pub charset: CharsetState,

    /// The most-recently-seen protected-mode kind. Port of
    /// `Screen.protected_mode`. Never reset to `.off` once set, because
    /// `eraseChars` and friends depend on knowing the most recent mode.
    pub protected_mode: ProtectedMode,

    /// The kitty keyboard settings.
    pub kitty_keyboard: FlagStack,

    /// Semantic prompt (OSC 133) state.
    pub semantic_prompt: SemanticPrompt,

    /// Dirty flags for the renderer.
    pub dirty: Dirty,

    /// Kitty graphics image storage is a separate chunk; a single dirty bool
    /// stands in for `kitty_images.dirty`.
    pub kitty_images_dirty: bool,
}

impl Screen {
    /// Initialize a new screen. Port of `init`.
    pub fn init(opts: Options) -> Screen {
        let mut pages = PageList::init(
            opts.cols,
            opts.rows,
            if opts.max_scrollback == 0 {
                None
            } else {
                Some(opts.max_scrollback)
            },
        );

        // Track a pin for the cursor at the first page's top-left.
        let page_pin = pages.track_pin(Pin::at(pages.head_node()));
        // SAFETY: the pin was just tracked and is valid.
        let (page_row, page_cell) = unsafe { (*page_pin).row_and_cell() };

        Screen {
            pages,
            no_scrollback: opts.max_scrollback == 0,
            cursor: Cursor::new(page_pin, page_row, page_cell),
            saved_cursor: None,
            selection: None,
            charset: CharsetState::default(),
            protected_mode: ProtectedMode::default(),
            kitty_keyboard: FlagStack::default(),
            semantic_prompt: SemanticPrompt::default(),
            dirty: Dirty::default(),
            kitty_images_dirty: false,
        }
    }

    /// Assert screen-local consistency (cursor pin vs. cached x/y). Port of
    /// `assertIntegrity`. Only compiled in debug builds.
    #[inline]
    pub fn assert_integrity(&self) {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.cursor.x < self.pages.cols());
            debug_assert!(self.cursor.y < self.pages.rows());

            // SAFETY: cursor pin is a live tracked pin.
            let pin = unsafe { *self.cursor.page_pin };
            let pt = self
                .pages
                .point_from_pin(Tag::Active, pin)
                .expect("cursor pin outside active area");
            debug_assert_eq!(self.cursor.x, pt.coord.x);
            debug_assert_eq!(self.cursor.y as u32, pt.coord.y);
        }
    }

    /// Reset per DEC RIS. Port of `reset`.
    pub fn reset(&mut self) {
        self.pages.reset();

        // The reset preserves tracked pins, so the cursor pin is still valid and
        // should be at top-left.
        let cursor_pin = self.cursor.page_pin;
        // SAFETY: cursor pin is live and, post-reset, at the first page.
        unsafe {
            debug_assert_eq!((*cursor_pin).node, self.pages.head_node());
            debug_assert_eq!((*cursor_pin).x(), 0);
            debug_assert_eq!((*cursor_pin).y(), 0);
            let (row, cell) = (*cursor_pin).row_and_cell();
            self.cursor = Cursor::new(cursor_pin, row, cell);
        }

        self.saved_cursor = None;
        self.charset = CharsetState::default();
        self.kitty_keyboard = FlagStack::default();
        self.semantic_prompt = SemanticPrompt::default();
        self.clear_selection();
    }

    /// Clone the screen for the region `[top, bot]`. Copies dimensions and cell
    /// data; does NOT copy kitty images or live hyperlink cursor state (matching
    /// Zig — clone is for read-only ops). The selection is preserved (and clipped
    /// to the cloned area) via the tracked-pin remap. Port of `clone`.
    pub fn clone(&self, top: Point, bot: Option<Point>) -> Screen {
        let mut remap: Vec<(*mut Pin, *mut Pin)> = Vec::new();
        let mut pages = self.pages.clone(top, bot, Some(&mut remap));

        // Find the cursor in the clone. If it isn't in the cloned area, move it
        // to the top-left (a screen must have SOME cursor).
        let cursor = {
            let remapped = remap
                .iter()
                .find(|(old, _)| *old == self.cursor.page_pin)
                .map(|(_, new)| *new);
            let mut chosen: Option<Cursor> = None;
            if let Some(p) = remapped {
                // SAFETY: p is a live tracked pin in `pages`.
                let pin = unsafe { *p };
                if let Some(pt) = pages.point_from_pin(Tag::Active, pin) {
                    // SAFETY: p live.
                    let (row, cell) = unsafe { (*p).row_and_cell() };
                    let mut c = Cursor::new(p, row, cell);
                    c.x = pt.coord.x;
                    c.y = pt.coord.y as CellCountInt;
                    chosen = Some(c);
                }
            }
            chosen.unwrap_or_else(|| {
                let page_pin = pages.track_pin(Pin::at(pages.head_node()));
                // SAFETY: just tracked.
                let (row, cell) = unsafe { (*page_pin).row_and_cell() };
                Cursor::new(page_pin, row, cell)
            })
        };

        // Preserve the selection if we have one, clipping to the cloned area.
        let remap_get = |old: *mut Pin| -> Option<*mut Pin> {
            remap.iter().find(|(o, _)| *o == old).map(|(_, n)| *n)
        };
        let selection: Option<Selection> = self.selection.and_then(|sel| {
            debug_assert!(sel.tracked());

            // Order the (tracked) endpoints tl/br by pin pointer.
            let (tl, br) = match sel.order(&self.pages) {
                Order::Forward | Order::MirroredForward => {
                    (sel.tracked_start().unwrap(), sel.tracked_end().unwrap())
                }
                Order::Reverse | Order::MirroredReverse => {
                    (sel.tracked_end().unwrap(), sel.tracked_start().unwrap())
                }
            };

            let start_pin: *mut Pin = match remap_get(tl) {
                Some(p) => p,
                None => {
                    // tl fell outside the clone.
                    if remap_get(br).is_none() {
                        // Either the whole selection is out of bounds or the
                        // clone is within the selection. Decide via the clone
                        // top's screen y.
                        let clone_top = self.pages.pin(top)?;
                        let clone_top_y = self
                            .pages
                            .point_from_pin(Tag::Screen, clone_top)
                            .unwrap()
                            .coord
                            .y;
                        // SAFETY: tracked pin pointers live.
                        let br_y = self
                            .pages
                            .point_from_pin(Tag::Screen, unsafe { *br })
                            .unwrap()
                            .coord
                            .y;
                        let tl_y = self
                            .pages
                            .point_from_pin(Tag::Screen, unsafe { *tl })
                            .unwrap()
                            .coord
                            .y;
                        if br_y < clone_top_y {
                            return None;
                        }
                        if tl_y > clone_top_y {
                            return None;
                        }
                    }
                    // Move the top pin back in bounds to the top row.
                    // SAFETY: tl pin pointer live.
                    let tl_x = unsafe { (*tl).x };
                    let mut p = Pin::at(pages.head_node());
                    if sel.rectangle {
                        p.x = tl_x;
                    }
                    pages.track_pin(p)
                }
            };

            // Move the bottom-right pin back in bounds if it isn't already.
            let end_pin: *mut Pin = match remap_get(br) {
                Some(p) => p,
                None => {
                    // SAFETY: br pin pointer live; last node live.
                    let br_x = unsafe { (*br).x };
                    let last = pages.last_node();
                    let last_rows = unsafe { pages.node_data(last).size.rows };
                    let x = if sel.rectangle {
                        br_x
                    } else {
                        pages.cols() - 1
                    };
                    let p = Pin::with(last, last_rows - 1, x);
                    pages.track_pin(p)
                }
            };

            Some(Selection::from_tracked(start_pin, end_pin, sel.rectangle))
        });

        let result = Screen {
            pages,
            no_scrollback: self.no_scrollback,
            cursor,
            saved_cursor: None,
            selection,
            charset: CharsetState::default(),
            protected_mode: ProtectedMode::default(),
            kitty_keyboard: FlagStack::default(),
            semantic_prompt: SemanticPrompt::default(),
            dirty: self.dirty,
            kitty_images_dirty: false,
        };
        result.assert_integrity();
        result
    }

    // ---- small helpers -------------------------------------------------

    /// The cursor's current page (mutable).
    ///
    /// # Safety
    /// The cursor pin must be live (always true for a tracked cursor pin).
    pub(crate) unsafe fn cursor_page(&self) -> *mut Page {
        unsafe {
            let node = (*self.cursor.page_pin).node;
            self.pages.node_page_ptr(node)
        }
    }

    /// Refresh the cached `page_row`/`page_cell` from the cursor pin.
    ///
    /// # Safety
    /// The cursor pin must be valid.
    unsafe fn refresh_cursor_pointers(&mut self) {
        unsafe {
            let (row, cell) = (*self.cursor.page_pin).row_and_cell();
            self.cursor.page_row = row;
            self.cursor.page_cell = cell;
        }
    }

    /// The blank cell to use when preserving the cursor bg. Port of `blankCell`.
    ///
    /// If the cursor's style has an explicit background color, blanked cells
    /// carry that background (a `BgColor*` cell); otherwise the plain default
    /// cell.
    pub(crate) fn blank_cell(&self) -> Cell {
        if self.cursor.style_id == style::DEFAULT_ID {
            return Cell::default();
        }
        self.cursor.style.bg_cell().unwrap_or_default()
    }

    /// The active dimensions.
    pub fn cols(&self) -> CellCountInt {
        self.pages.cols()
    }
    pub fn rows(&self) -> CellCountInt {
        self.pages.rows()
    }

    // ---- cursor motion (fast paths) ------------------------------------

    /// Move the cursor right by `n` (no wrapping). Port of `cursorRight`.
    pub fn cursor_right(&mut self, n: CellCountInt) {
        debug_assert!(self.cursor.x + n < self.pages.cols());
        // SAFETY: bounds asserted; the cell pointer stays within the row.
        unsafe {
            self.cursor.page_cell = self.cursor.page_cell.add(n as usize);
            (*self.cursor.page_pin).x += n;
        }
        self.cursor.x += n;
        self.assert_integrity();
    }

    /// Move the cursor left by `n`. Port of `cursorLeft`.
    pub fn cursor_left(&mut self, n: CellCountInt) {
        debug_assert!(self.cursor.x >= n);
        // SAFETY: bounds asserted.
        unsafe {
            self.cursor.page_cell = self.cursor.page_cell.sub(n as usize);
            (*self.cursor.page_pin).x -= n;
        }
        self.cursor.x -= n;
        self.assert_integrity();
    }

    /// The cell `n` to the right of the cursor. Port of `cursorCellRight`.
    /// Retained as a faithful port of the Screen API; consumed once the print
    /// pipeline (Terminal chunk) lands.
    ///
    /// # Safety
    /// `cursor.x + n < cols`.
    #[allow(dead_code)]
    pub(crate) unsafe fn cursor_cell_right(&self, n: CellCountInt) -> *mut Cell {
        debug_assert!(self.cursor.x + n < self.pages.cols());
        unsafe { self.cursor.page_cell.add(n as usize) }
    }

    /// The cell `n` to the left of the cursor. Port of `cursorCellLeft`.
    ///
    /// # Safety
    /// `cursor.x >= n`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unsafe fn cursor_cell_left(&self, n: CellCountInt) -> *mut Cell {
        debug_assert!(self.cursor.x >= n);
        unsafe { self.cursor.page_cell.sub(n as usize) }
    }

    /// Move the cursor up by `n`. Port of `cursorUp`.
    pub fn cursor_up(&mut self, n: CellCountInt) {
        debug_assert!(self.cursor.y >= n);
        self.cursor.y -= n; // must precede cursor_change_pin
        // SAFETY: cursor pin valid; up(n) succeeds because y >= n.
        let new = unsafe { (*self.cursor.page_pin).up(n as usize).unwrap() };
        self.cursor_change_pin(new);
        // SAFETY: cursor pin valid after change.
        unsafe { self.refresh_cursor_pointers() };
        self.assert_integrity();
    }

    /// The row `n` rows above the cursor, without moving the cursor. Port of
    /// `cursorRowUp` — used by `Terminal::cursor_left`'s reverse-wrap logic to
    /// inspect the previous row's `wrap` flag.
    ///
    /// # Safety
    /// The caller must guarantee `cursor.y >= n` (a row exists `n` above).
    pub(crate) unsafe fn cursor_row_up(&self, n: CellCountInt) -> *mut crate::page::Row {
        debug_assert!(self.cursor.y >= n);
        // SAFETY: cursor pin valid; up(n) succeeds because y >= n (caller-asserted).
        unsafe {
            let pin = (*self.cursor.page_pin).up(n as usize).unwrap();
            let (row, _) = pin.row_and_cell();
            row
        }
    }

    /// Move the cursor down by `n`. Port of `cursorDown`.
    pub fn cursor_down(&mut self, n: CellCountInt) {
        debug_assert!(self.cursor.y + n < self.pages.rows());
        self.cursor.y += n; // must precede cursor_change_pin
        // SAFETY: cursor pin valid; down(n) succeeds because y+n < rows.
        let new = unsafe { (*self.cursor.page_pin).down(n as usize).unwrap() };
        self.cursor_change_pin(new);
        // SAFETY: cursor pin valid after change.
        unsafe { self.refresh_cursor_pointers() };
        self.assert_integrity();
    }

    /// Move the cursor to an absolute column. Port of `cursorHorizontalAbsolute`.
    pub fn cursor_horizontal_absolute(&mut self, x: CellCountInt) {
        debug_assert!(x < self.pages.cols());
        // SAFETY: cursor pin valid; x in bounds.
        unsafe {
            (*self.cursor.page_pin).x = x;
            let (_, cell) = (*self.cursor.page_pin).row_and_cell();
            self.cursor.page_cell = cell;
        }
        self.cursor.x = x;
        self.assert_integrity();
    }

    /// Move the cursor to an absolute position. Port of `cursorAbsolute`.
    pub fn cursor_absolute(&mut self, x: CellCountInt, y: CellCountInt) {
        debug_assert!(x < self.pages.cols());
        debug_assert!(y < self.pages.rows());

        // SAFETY: cursor pin valid; up/down within bounds by the y comparison.
        let mut page_pin = unsafe {
            use std::cmp::Ordering;
            match y.cmp(&self.cursor.y) {
                Ordering::Less => (*self.cursor.page_pin)
                    .up((self.cursor.y - y) as usize)
                    .unwrap(),
                Ordering::Greater => (*self.cursor.page_pin)
                    .down((y - self.cursor.y) as usize)
                    .unwrap(),
                Ordering::Equal => *self.cursor.page_pin,
            }
        };
        page_pin.x = x;
        self.cursor.x = x; // must precede cursor_change_pin
        self.cursor.y = y;
        self.cursor_change_pin(page_pin);
        // SAFETY: cursor pin valid after change.
        unsafe { self.refresh_cursor_pointers() };
        self.assert_integrity();
    }

    /// Expensive recovery: rebuild all cached cursor state from the pin. Port of
    /// `cursorReload`.
    pub fn cursor_reload(&mut self) {
        // The tracked pin is always accurate. Derive the active point; if the
        // pin points outside the active area, repoint it to the active top-left.
        // SAFETY: cursor pin is live.
        let pt = unsafe {
            match self
                .pages
                .point_from_pin(Tag::Active, *self.cursor.page_pin)
            {
                Some(pt) => pt,
                None => {
                    let pin = self.pages.pin(Point::active(0, 0)).unwrap();
                    *self.cursor.page_pin = pin;
                    self.pages.point_from_pin(Tag::Active, pin).unwrap()
                }
            }
        };

        self.cursor.x = pt.coord.x;
        self.cursor.y = pt.coord.y as CellCountInt;
        // SAFETY: cursor pin valid.
        unsafe { self.refresh_cursor_pointers() };

        // Re-intern the style since the page may have changed.
        if self.cursor.style_id != style::DEFAULT_ID && self.manual_style_update().is_err() {
            self.cursor.style = Style::default();
            self.cursor.style_id = 0;
        }
        self.assert_integrity();
    }

    /// Mark the cursor row dirty. Port of `cursorMarkDirty`.
    #[inline]
    pub fn cursor_mark_dirty(&mut self) {
        // SAFETY: cursor page_row is a cached live pointer.
        unsafe {
            (*self.cursor.page_row).set_dirty(true);
        }
    }

    /// The ONLY sanctioned writer of `cursor.page_pin`. Migrates style/hyperlink
    /// across pages and marks dirty. Port of `cursorChangePin`.
    fn cursor_change_pin(&mut self, new: Pin) {
        // Moving the cursor affects run-splitting (ligatures): dirty both rows.
        // SAFETY: both pins are valid.
        unsafe {
            if !(*self.cursor.page_pin).eql(new) {
                self.cursor_mark_dirty();
                new.mark_dirty();
            }
        }

        // Same page: just update the pin, no state migration.
        if unsafe { (*self.cursor.page_pin).node } == new.node {
            unsafe {
                *self.cursor.page_pin = new;
            }
            return;
        }

        // Release the old style from the old page (directly, because the cursor
        // position may already have moved but the pin hasn't).
        let old_style = if self.cursor.style_id == style::DEFAULT_ID {
            None
        } else {
            Some(self.cursor.style)
        };
        if old_style.is_some() {
            // SAFETY: cursor page live; style_id valid in it.
            unsafe {
                let page = self.cursor_page();
                let mem = (*page).memory_mut();
                (*page).styles().release(mem, self.cursor.style_id);
            }
            self.cursor.style = Style::default();
            self.cursor.style_id = style::DEFAULT_ID;
        }

        // Release the old hyperlink from the old page.
        if self.cursor.hyperlink.is_some() {
            // SAFETY: cursor page live; hyperlink_id valid in it.
            unsafe {
                let page = self.cursor_page();
                let mem = (*page).memory_mut();
                (*page)
                    .hyperlink_set_mut()
                    .release(mem, self.cursor.hyperlink_id);
            }
        }

        // Move to the new page.
        // SAFETY: cursor pin live.
        unsafe {
            *self.cursor.page_pin = new;
        }

        // Migrate the style onto the new page.
        if let Some(s) = old_style {
            self.cursor.style = s;
            if self.manual_style_update().is_err() {
                self.cursor.style = Style::default();
                self.cursor.style_id = 0;
            }
        }

        // Migrate the hyperlink onto the new page.
        if let Some(link) = self.cursor.hyperlink.take() {
            self.cursor.hyperlink_id = 0;
            let id = link.explicit_id().map(|s| s.to_vec());
            let _ = self.start_hyperlink(&link.uri, id.as_deref());
        }
    }

    // ---- style caching -------------------------------------------------

    /// Re-intern `cursor.style` on the cursor's current page, releasing the old
    /// id. Port of `manualStyleUpdate`. Returns `Err` only on unrecoverable OOM.
    #[allow(clippy::result_unit_err)]
    pub fn manual_style_update(&mut self) -> Result<(), ()> {
        // SAFETY: cursor page live throughout.
        unsafe {
            let page = self.cursor_page();
            let mem = (*page).memory_mut();

            // Release the previous non-default style.
            if self.cursor.style_id != style::DEFAULT_ID {
                (*page).styles().release(mem, self.cursor.style_id);
            }

            // Default style: reset to id 0.
            if self.cursor.style.is_default() {
                self.cursor.style_id = style::DEFAULT_ID;
                self.assert_integrity();
                return Ok(());
            }

            // Clear id first so a capacity adjustment (which re-enters here)
            // falls back to the default cleanly.
            self.cursor.style_id = style::DEFAULT_ID;

            let value = self.cursor.style;
            match (*page).styles().add(mem, value) {
                Ok(id) => {
                    self.cursor.style_id = id;
                    self.assert_integrity();
                    Ok(())
                }
                Err(_) => {
                    // Style map full or needs rehash: grow style capacity (or
                    // rehash), or split the page on OutOfSpace, then retry.
                    let node = (*self.cursor.page_pin).node;
                    match self.increase_capacity(node, Some(IncreaseCapacity::Styles)) {
                        Ok(_) => {}
                        Err(()) => {
                            // OutOfSpace: split the page and retry on the (new)
                            // cursor page.
                            let pin = *self.cursor.page_pin;
                            if self.split_for_capacity(pin).is_err() {
                                return Err(());
                            }
                        }
                    }

                    let page = self.cursor_page();
                    let mem = (*page).memory_mut();
                    match (*page).styles().add(mem, value) {
                        Ok(id) => {
                            self.cursor.style_id = id;
                            self.assert_integrity();
                            Ok(())
                        }
                        Err(_) => Err(()),
                    }
                }
            }
        }
    }

    /// Raw wrapper over `PageList::increase_capacity`.
    ///
    /// # Safety-adjacent
    /// `node` must be live. Returns `Err(())` on OutOfSpace.
    fn increase_capacity_raw(
        &mut self,
        node: *mut crate::pagelist::Node,
        adjustment: Option<IncreaseCapacity>,
    ) -> Result<*mut crate::pagelist::Node, ()> {
        // SAFETY: node is live (a cursor/tracked-pin node).
        unsafe { self.pages.increase_capacity(node, adjustment) }
    }

    /// Screen's wrapper over `PageList::increase_capacity` that re-adds the
    /// cursor style/hyperlink when the cursor's own page is reallocated. Port of
    /// `increaseCapacity`. Returns `Err(())` on OutOfSpace.
    fn increase_capacity(
        &mut self,
        node: *mut crate::pagelist::Node,
        adjustment: Option<IncreaseCapacity>,
    ) -> Result<*mut crate::pagelist::Node, ()> {
        // If not the cursor page, it's a plain operation (increase_capacity
        // updates all tracked pins, including the cursor, already).
        // SAFETY: cursor pin live.
        if node != unsafe { (*self.cursor.page_pin).node } {
            return self.increase_capacity_raw(node, adjustment);
        }

        // Cursor page: after realloc, re-add the cursor style and hyperlink.
        let new_node = self.increase_capacity_raw(node, adjustment)?;

        // Re-add the style.
        if self.cursor.style_id != style::DEFAULT_ID {
            // SAFETY: new_node live.
            let added = unsafe {
                let page = self.pages.node_data_mut(new_node);
                let mem = (*page).memory_mut();
                (*page).styles().add(mem, self.cursor.style)
            };
            match added {
                Ok(id) => self.cursor.style_id = id,
                Err(_) => {
                    self.cursor.style = Style::default();
                    self.cursor.style_id = style::DEFAULT_ID;
                }
            }
        }

        // Re-add the hyperlink.
        if let Some(link) = self.cursor.hyperlink.take() {
            self.cursor.hyperlink_id = 0;
            let id = link.explicit_id().map(|s| s.to_vec());
            let _ = self.start_hyperlink_once(&Hyperlink {
                uri: link.uri.clone(),
                id: match id {
                    Some(v) => HyperlinkId::Explicit(v),
                    None => link.id.clone(),
                },
            });
        }

        // Reload the cursor since the pin changed.
        self.cursor_reload();
        Ok(new_node)
    }

    /// Split the cursor page at `pin` so the pinned row lands on the page with
    /// less used capacity. Port of `splitForCapacity`.
    fn split_for_capacity(&mut self, pin: Pin) -> Result<(), ()> {
        // SAFETY: pin is the cursor pin (live).
        let (bytes_above, bytes_below) = unsafe {
            let page = pin.page();
            let cap_above = (*page).exact_row_capacity(0, pin.y() as usize + 1);
            let cap_below =
                (*page).exact_row_capacity(pin.y() as usize, (*page).size.rows as usize);
            (
                crate::page::layout_total_size(cap_above),
                crate::page::layout_total_size(cap_below),
            )
        };

        // SAFETY: cursor pin live.
        let old_cursor = unsafe { *self.cursor.page_pin };

        let split_at = if bytes_above < bytes_below {
            // SAFETY: pin live.
            unsafe { pin.down(1).unwrap_or(pin) }
        } else {
            pin
        };

        if self.pages.split(split_at).is_err() {
            return Err(());
        }

        // Cursor didn't change nodes: done.
        // SAFETY: cursor pin live.
        if unsafe { (*self.cursor.page_pin).node } == old_cursor.node {
            return Ok(());
        }

        // Restore the old pin then move via cursor_change_pin.
        // SAFETY: cursor pin live.
        let new_cursor = unsafe { *self.cursor.page_pin };
        unsafe {
            *self.cursor.page_pin = old_cursor;
        }
        self.cursor_change_pin(new_cursor);
        Ok(())
    }

    // ---- hyperlink caching ---------------------------------------------

    /// Start OSC 8 hyperlink state. Port of `startHyperlink`.
    #[allow(clippy::result_unit_err)]
    pub fn start_hyperlink(&mut self, uri: &[u8], id: Option<&[u8]>) -> Result<(), ()> {
        let link = Hyperlink {
            uri: uri.to_vec(),
            id: match id {
                Some(idb) => HyperlinkId::Explicit(idb.to_vec()),
                None => {
                    let v = self.cursor.hyperlink_implicit_id;
                    self.cursor.hyperlink_implicit_id =
                        self.cursor.hyperlink_implicit_id.wrapping_add(1);
                    HyperlinkId::Implicit(v)
                }
            },
        };

        loop {
            match self.start_hyperlink_once(&link) {
                Ok(()) => return Ok(()),
                Err(InsertHyperlinkError::StringsOutOfMemory) => {
                    // SAFETY: cursor pin live.
                    let node = unsafe { (*self.cursor.page_pin).node };
                    if self
                        .increase_capacity(node, Some(IncreaseCapacity::StringBytes))
                        .is_err()
                    {
                        return Err(());
                    }
                }
                Err(InsertHyperlinkError::SetOutOfMemory) => {
                    let node = unsafe { (*self.cursor.page_pin).node };
                    if self
                        .increase_capacity(node, Some(IncreaseCapacity::HyperlinkBytes))
                        .is_err()
                    {
                        return Err(());
                    }
                }
                Err(InsertHyperlinkError::SetNeedsRehash) => {
                    let node = unsafe { (*self.cursor.page_pin).node };
                    if self.increase_capacity(node, None).is_err() {
                        return Err(());
                    }
                }
            }
            self.assert_integrity();
        }
    }

    fn start_hyperlink_once(&mut self, source: &Hyperlink) -> Result<(), InsertHyperlinkError> {
        self.end_hyperlink();

        let insert_id = match &source.id {
            HyperlinkId::Explicit(idb) => HyperlinkInsertId::Explicit(idb),
            HyperlinkId::Implicit(v) => HyperlinkInsertId::Implicit(*v),
        };

        // SAFETY: cursor page live.
        let id = unsafe {
            let page = self.cursor_page();
            (*page).insert_hyperlink(&source.uri, insert_id)?
        };

        self.cursor.hyperlink = Some(Box::new(source.clone()));
        self.cursor.hyperlink_id = id;
        Ok(())
    }

    /// End OSC 8 hyperlink state. Idempotent. Port of `endHyperlink`.
    pub fn end_hyperlink(&mut self) {
        if self.cursor.hyperlink_id == 0 {
            debug_assert!(self.cursor.hyperlink.is_none());
            return;
        }
        // SAFETY: cursor page live; hyperlink_id valid in it.
        unsafe {
            let page = self.cursor_page();
            let mem = (*page).memory_mut();
            (*page)
                .hyperlink_set_mut()
                .release(mem, self.cursor.hyperlink_id);
        }
        self.cursor.hyperlink = None;
        self.cursor.hyperlink_id = 0;
    }

    // ---- scroll --------------------------------------------------------

    /// Scroll the viewport. Port of `scroll`.
    pub fn scroll(&mut self, behavior: Scroll) {
        self.kitty_images_dirty = true;
        match behavior {
            Scroll::Active => self.pages.scroll(PageScroll::Active),
            Scroll::Top => self.pages.scroll(PageScroll::Top),
            Scroll::Pin(p) => self.pages.scroll(PageScroll::Pin(p)),
            Scroll::Row(v) => self.pages.scroll(PageScroll::Row(v)),
            Scroll::DeltaRow(v) => self.pages.scroll(PageScroll::DeltaRow(v)),
            Scroll::DeltaPrompt(v) => self.pages.scroll(PageScroll::DeltaPrompt(v)),
        }
        self.assert_integrity();
    }

    /// Scroll and clear; reset the cursor to the top. Port of `scrollClear`.
    pub fn scroll_clear(&mut self) {
        self.pages.scroll_clear();
        self.cursor_reload();
        self.kitty_images_dirty = true;
        self.assert_integrity();
    }

    /// True if the viewport is at the bottom. Port of `viewportIsBottom`.
    pub fn viewport_is_bottom(&self) -> bool {
        self.pages.viewport_is_active()
    }

    /// Scroll the active area, keeping the cursor at the bottom. Port of
    /// `cursorDownScroll`. Precondition: cursor on the last active row.
    pub fn cursor_down_scroll(&mut self) {
        debug_assert_eq!(self.cursor.y, self.pages.rows() - 1);
        self.kitty_images_dirty = true;

        if self.no_scrollback {
            if self.pages.rows() == 1 {
                // Single row: just clear it.
                // SAFETY: cursor row/page live.
                unsafe {
                    let page = self.cursor_page();
                    let cols = (*page).size.cols as usize;
                    let blank = self.blank_cell();
                    (*page).fill_cells(self.cursor.page_row, 0, cols, blank);
                }
                self.cursor_mark_dirty();
            } else {
                // eraseRow shifts everything below up (and moves the cursor pin
                // up by one, which we undo).
                // SAFETY: cursor pin live.
                let old_pin = unsafe { *self.cursor.page_pin };
                self.pages.erase_row(Point::active(0, 0));
                // SAFETY: cursor pin live; restore its position.
                unsafe {
                    *self.cursor.page_pin = old_pin;
                    self.refresh_cursor_pointers();
                }
            }
        } else {
            // SAFETY: cursor pin live.
            let old_pin = unsafe { *self.cursor.page_pin };
            let _ = self.pages.grow();

            // Compute the new pin. If our page changed, grow() pruned and moved
            // the pin to the new page top-left (already +1 row); else move down.
            // SAFETY: cursor pin live.
            let new_pin = unsafe {
                if old_pin.node == (*self.cursor.page_pin).node {
                    (*self.cursor.page_pin).down(1).unwrap()
                } else {
                    let mut pin = *self.cursor.page_pin;
                    pin.x = self.cursor.x;
                    pin
                }
            };
            self.cursor_change_pin(new_pin);
            // SAFETY: cursor pin live.
            unsafe { self.refresh_cursor_pointers() };
            self.cursor_mark_dirty();
        }
        self.assert_integrity();
    }

    /// Move down if not at the bottom, else scroll. Port of `cursorDownOrScroll`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cursor_down_or_scroll(&mut self) {
        if self.cursor.y + 1 < self.pages.rows() {
            self.cursor_down(1);
        } else {
            self.cursor_down_scroll();
        }
    }

    /// Insert a blank row above the cursor: everything below the cursor shifts
    /// down by one and a new blank row appears at the cursor. Port of
    /// `cursorScrollAbove`. Creates scrollback (`grow`) rather than shifting the
    /// whole scrollback up.
    pub(crate) fn cursor_scroll_above(&mut self) {
        // We unconditionally mark the cursor row dirty because the cursor
        // always changes page rows here and that can force re-shaping.
        self.cursor_mark_dirty();

        // Bottom of the screen: use the specialized fast path.
        if self.cursor.y == self.pages.rows() - 1 {
            self.cursor_down_scroll();
            return;
        }

        // Logic below assumes at least one row that isn't moving.
        debug_assert!(self.cursor.y < self.pages.rows() - 1);

        // SAFETY: cursor pin live.
        let old_pin = unsafe { *self.cursor.page_pin };
        if self.pages.grow_node().is_some() {
            self.cursor_scroll_above_rotate();
        } else {
            // grow() didn't allocate a new page.
            // SAFETY: cursor pin live.
            let on_last = unsafe { (*self.cursor.page_pin).node } == self.pages.last_node();
            if on_last {
                // Fast path: all the rows we move are within one page.
                debug_assert_eq!(old_pin.node, unsafe { (*self.cursor.page_pin).node });
                // SAFETY: cursor pin live; down(1) valid (not on last row).
                unsafe {
                    *self.cursor.page_pin = (*self.cursor.page_pin).down(1).unwrap();
                    let pin = *self.cursor.page_pin;
                    let page: *mut Page = self.pages.node_page_ptr(pin.node);
                    let y = pin.y as usize;
                    let rows = (*page).size.rows as usize;
                    (*page).pause_integrity_checks(true);
                    (*page).rotate_rows_once_right(y, rows);
                    (*page).pause_integrity_checks(false);
                    (*page).dirty = true;
                    let (row, cell) = pin.row_and_cell();
                    self.cursor.page_row = row;
                    self.cursor.page_cell = cell;
                }
            } else {
                // Didn't grow pages but cursor isn't on the last page: copy
                // rows across pages (slow path).
                self.cursor_scroll_above_rotate();
            }
        }

        // The newly created line needs to be styled per the bg color if set.
        if self.cursor.style_id != style::DEFAULT_ID
            && let Some(blank) = self.cursor.style.bg_cell()
        {
            // SAFETY: cursor page/row live; fill the whole row.
            unsafe {
                let page = self.cursor_page();
                let cols = (*page).size.cols as usize;
                let base = (*(self.cursor.page_row)).cells().ptr((*page).mem());
                for x in 0..cols {
                    base.add(x).write(blank);
                }
            }
        }
        self.assert_integrity();
    }

    /// The cross-page rotate helper for [`Self::cursor_scroll_above`]. Port of
    /// `cursorScrollAboveRotate`.
    fn cursor_scroll_above_rotate(&mut self) {
        // SAFETY: cursor pin live; down(1) valid.
        let new = unsafe { (*self.cursor.page_pin).down(1).unwrap() };
        self.cursor_change_pin(new);

        // Go through each page following our pin, shift all rows down by one
        // and copy the last row of the previous page.
        // SAFETY: node chain live.
        unsafe {
            let cursor_node = (*self.cursor.page_pin).node;
            let mut current = self.pages.last_node();
            while current != cursor_node {
                let prev = (*current).prev;
                let prev_page: *mut Page = self.pages.node_page_ptr(prev);
                let cur_page: *mut Page = self.pages.node_page_ptr(current);
                let cur_rows = (*cur_page).size.rows as usize;

                (*cur_page).pause_integrity_checks(true);
                (*cur_page).rotate_rows_once_right(0, cur_rows);
                (*cur_page).pause_integrity_checks(false);

                // Copy the last row of prev to the top of current.
                let dst_row = (*cur_page).get_row(0);
                let src_row = (*prev_page).get_row((*prev_page).size.rows as usize - 1);
                let _ = (*cur_page).clone_row_from(prev_page, dst_row, src_row);
                (*cur_page).dirty = true;

                current = prev;
            }

            // current is the cursor page: rotate from the cursor row down and
            // clear the cursor row.
            debug_assert_eq!(current, (*self.cursor.page_pin).node);
            let cur_page: *mut Page = self.pages.node_page_ptr(current);
            let y = (*self.cursor.page_pin).y as usize;
            let cur_rows = (*cur_page).size.rows as usize;
            (*cur_page).pause_integrity_checks(true);
            (*cur_page).rotate_rows_once_right(y, cur_rows);
            (*cur_page).pause_integrity_checks(false);

            let row = (*cur_page).get_row(y);
            let cols = (*cur_page).size.cols as usize;
            self.clear_cells_page(cur_page, row, 0, cols);
            (*cur_page).dirty = true;

            // Refresh cursor caches after rotations.
            let (row, cell) = (*self.cursor.page_pin).row_and_cell();
            self.cursor.page_row = row;
            self.cursor.page_cell = cell;
        }
    }

    /// Copy another cursor into this screen. The source cursor may be on any
    /// screen, but its x/y must be within our bounds. Port of `cursorCopy` (the
    /// `hyperlink = false` path used by alt-screen switching).
    pub(crate) fn cursor_copy(&mut self, other: &CursorCopy) {
        debug_assert!(other.x < self.pages.cols());
        debug_assert!(other.y < self.pages.rows());

        // End any active hyperlink on our cursor.
        self.end_hyperlink();

        // Adopt the other cursor's value state, but keep our own page pin, x/y,
        // and (old) style id so the old style is cleaned up in our own page.
        let old_style_id = self.cursor.style_id;
        self.cursor.style = other.style;
        self.cursor.protected = other.protected;
        self.cursor.pending_wrap = other.pending_wrap;
        self.cursor.cursor_style = other.cursor_style;
        self.cursor.semantic_content = other.semantic_content;
        self.cursor.semantic_content_clear_eol = other.semantic_content_clear_eol;
        self.cursor.style_id = old_style_id;

        // Clean up our old style and load the other cursor's style.
        if self.manual_style_update().is_err() {
            self.cursor.style = Style::default();
            self.cursor.style_id = style::DEFAULT_ID;
        }

        // Move to the target location within our own screen.
        self.cursor_absolute(other.x, other.y);
        self.assert_integrity();
    }

    /// Reset the cursor row's soft-wrap state. Port of `cursorResetWrap`.
    pub(crate) fn cursor_reset_wrap(&mut self) {
        self.cursor.pending_wrap = false;

        // SAFETY: cursor row live.
        if !unsafe { (*self.cursor.page_row).wrap() } {
            return;
        }

        // This row does not wrap and the next row is not wrapped-to.
        // SAFETY: cursor row live.
        unsafe {
            (*self.cursor.page_row).set_wrap(false);
        }
        // SAFETY: cursor pin live.
        if let Some(next) = unsafe { (*self.cursor.page_pin).down(1) } {
            // SAFETY: next pin valid.
            unsafe {
                let (row, _) = next.row_and_cell();
                (*row).set_wrap_continuation(false);
            }
        }

        // If the last cell is a spacer head, clear it.
        // SAFETY: cursor page/row live.
        unsafe {
            let page = self.cursor_page();
            let cols = (*page).size.cols as usize;
            let base = (*self.cursor.page_row).cells().ptr((*page).mem());
            if (*base.add(cols - 1)).wide() == crate::page::Wide::SpacerHead {
                self.clear_cells_page(page, self.cursor.page_row, cols - 1, cols);
            }
        }
    }

    /// Handle boundary conditions when splitting cells at column `x`. Port of
    /// `splitCellBoundary`. `x` may be up to and including `cols` (the boundary
    /// to the right of the final cell).
    pub(crate) fn split_cell_boundary(&mut self, x: CellCountInt) {
        use crate::page::Wide;
        // SAFETY: cursor page live.
        let page = unsafe { self.cursor_page() };
        // SAFETY: page live.
        let cols = unsafe { (*page).size.cols };
        debug_assert!(x <= cols);

        // SAFETY: page/row/cursor pin live throughout.
        unsafe {
            (*page).pause_integrity_checks(true);
            let base = (*self.cursor.page_row).cells().ptr((*page).mem());

            // [ A B C D E F | ]  boundary between final cell and row end.
            if x == cols {
                if !(*self.cursor.page_row).wrap() {
                    (*page).pause_integrity_checks(false);
                    return;
                }
                if (*base.add(cols as usize - 1)).wide() == Wide::SpacerHead {
                    self.clear_cells_page(
                        page,
                        self.cursor.page_row,
                        cols as usize - 1,
                        cols as usize,
                    );
                }
                (*page).pause_integrity_checks(false);
                return;
            }

            // [ | A B C ... ] or [ A | B C ... ] and the row is a wrap
            // continuation: the first cell may be a wide cell whose spacer head
            // lives on the previous row.
            if (x == 0 || x == 1)
                && (*self.cursor.page_row).wrap_continuation()
                && (*base.add(0)).wide() == Wide::Wide
                && let Some(p_row) = (*self.cursor.page_pin).up(1)
            {
                let p_page: *mut Page = self.pages.node_page_ptr(p_row.node);
                let (prow, _) = p_row.row_and_cell();
                let p_cols = (*p_page).size.cols as usize;
                let p_base = (*prow).cells().ptr((*p_page).mem());
                if (*p_base.add(p_cols - 1)).wide() == Wide::SpacerHead {
                    self.clear_cells_page(p_page, prow, p_cols - 1, p_cols);
                }
            }

            if x == 0 {
                (*page).pause_integrity_checks(false);
                return;
            }

            // [ ... X | Y ... ] boundary between two cells mid-row.
            debug_assert!(x > 0 && x < cols);
            match (*base.add(x as usize - 1)).wide() {
                Wide::SpacerHead => unreachable!("spacer head in the middle of a row"),
                Wide::Narrow | Wide::SpacerTail => {}
                Wide::Wide => {
                    self.clear_cells_page(
                        page,
                        self.cursor.page_row,
                        x as usize - 1,
                        x as usize + 1,
                    );
                }
            }
            (*page).pause_integrity_checks(false);
        }
    }

    // ---- clear / erase -------------------------------------------------

    /// Physically erase history. Port of `eraseHistory`.
    pub fn erase_history(&mut self, bl: Option<Point>) {
        self.pages.erase_history(bl);
        self.cursor_reload();
        self.assert_integrity();
    }

    /// Physically erase the active area from row `y`. Port of `eraseActive`.
    pub fn erase_active(&mut self, y: CellCountInt) {
        self.pages.erase_active(y);
        self.cursor_reload();
        self.assert_integrity();
    }

    /// Clear a region of rows (bg-colored blanks). Port of `clearRows`.
    pub fn clear_rows(&mut self, tl: Point, bl: Option<Point>, protected: bool) {
        let blank = self.blank_cell();
        let mut it = self.pages.row_iterator(Direction::RightDown, tl, bl);
        // SAFETY: iterator yields valid pins into live pages.
        unsafe {
            while let Some(pin) = it.next() {
                let node = pin.node;
                let page: *mut Page = self.pages.node_page_ptr(node);
                let (row, _) = pin.row_and_cell();
                let cols = (*page).size.cols as usize;
                if protected {
                    self.clear_unprotected_cells_page(page, row, 0, cols);
                } else {
                    (*page).fill_cells(row, 0, cols, blank);
                }
                (*row).set_dirty(true);
            }
        }
        self.assert_integrity();
    }

    /// Clear cells `[left, end)` of a row with the bg-blank. Port of `clearCells`
    /// (the release work lives in `Page::fill_cells`).
    ///
    /// # Safety
    /// `page`/`row` live; `[left, end)` in bounds.
    pub(crate) unsafe fn clear_cells_page(
        &self,
        page: *mut Page,
        row: *mut crate::page::Row,
        left: usize,
        end: usize,
    ) {
        let blank = self.blank_cell();
        unsafe {
            (*page).fill_cells(row, left, end, blank);
        }
    }

    /// Clear only unprotected cells within `[left, end)`. Port of
    /// `clearUnprotectedCells`.
    ///
    /// # Safety
    /// `page`/`row` live; range in bounds.
    pub(crate) unsafe fn clear_unprotected_cells_page(
        &self,
        page: *mut Page,
        row: *mut crate::page::Row,
        left: usize,
        end: usize,
    ) {
        unsafe {
            let cells = (*page).get_cells(row);
            let base = cells.cast::<Cell>();
            let mut x0 = left;
            while x0 < end {
                while (*base.add(x0)).protected() {
                    x0 += 1;
                    if x0 >= end {
                        return;
                    }
                }
                let mut x1 = x0 + 1;
                while x1 < end && !(*base.add(x1)).protected() {
                    x1 += 1;
                }
                self.clear_cells_page(page, row, x0, x1);
                x0 = x1;
            }
        }
    }

    // ---- semantic content ----------------------------------------------

    /// Set the cursor's semantic content. Port of `cursorSetSemanticContent`.
    pub fn cursor_set_semantic_content(&mut self, t: SemanticContentSet) {
        match t {
            SemanticContentSet::Output => {
                self.cursor.semantic_content = SemanticContent::Output;
                self.cursor.semantic_content_clear_eol = false;
            }
            SemanticContentSet::Input { clear_eol } => {
                self.cursor.semantic_content = SemanticContent::Input;
                self.cursor.semantic_content_clear_eol = clear_eol;
            }
            SemanticContentSet::Prompt(kind) => {
                self.semantic_prompt.seen = true;
                self.cursor.semantic_content = SemanticContent::Prompt;
                self.cursor.semantic_content_clear_eol = false;
                let sp = match kind {
                    PromptKind::Initial | PromptKind::Right => RowSemanticPrompt::Prompt,
                    PromptKind::Continuation | PromptKind::Secondary => {
                        RowSemanticPrompt::PromptContinuation
                    }
                };
                // SAFETY: cursor row live.
                unsafe {
                    (*self.cursor.page_row).set_semantic_prompt(sp);
                }
            }
        }
    }

    // ---- selection -----------------------------------------------------

    /// Set the selection to the given selection. If it is untracked it is
    /// tracked (taking ownership); the prior selection (if any) is untracked.
    /// `None` clears the selection. Port of `select`.
    ///
    /// The Zig version threads `Allocator.Error`; the Rust pin model is
    /// infallible-alloc, so this cannot fail.
    pub fn select(&mut self, sel: Option<Selection>) {
        let sel = match sel {
            None => {
                self.clear_selection();
                return;
            }
            Some(s) => s,
        };

        // If this selection is untracked then we track it.
        let tracked_sel = if sel.tracked() {
            sel
        } else {
            sel.track(&mut self.pages)
        };

        // Untrack the prior selection.
        if let Some(old) = self.selection.take() {
            old.deinit(&mut self.pages);
        }
        self.selection = Some(tracked_sel);
        self.dirty.selection = true;
    }

    /// Same as `select(None)` but can't fail. Port of `clearSelection`.
    pub fn clear_selection(&mut self) {
        if let Some(sel) = self.selection.take() {
            sel.deinit(&mut self.pages);
            self.dirty.selection = true;
        }
    }

    /// The selection for all contents on the screen, whitespace-trimmed, or
    /// `None` if the screen is empty. Port of `selectAll`.
    pub fn select_all(&self) -> Option<Selection> {
        const WHITESPACE: [u32; 3] = [0, ' ' as u32, '\t' as u32];

        let start = {
            let mut it = self
                .pages
                .cell_iterator(Direction::RightDown, Point::screen(0, 0), None);
            let mut found = None;
            // SAFETY: iterator yields valid pins into live pages.
            unsafe {
                while let Some(p) = it.next() {
                    let cell = *p.row_and_cell().1;
                    if !cell.has_text() {
                        continue;
                    }
                    if WHITESPACE.contains(&cell.codepoint()) {
                        continue;
                    }
                    found = Some(p);
                    break;
                }
            }
            found?
        };

        let end = {
            let mut it = self
                .pages
                .cell_iterator(Direction::LeftUp, Point::screen(0, 0), None);
            let mut found = None;
            // SAFETY: iterator yields valid pins into live pages.
            unsafe {
                while let Some(p) = it.next() {
                    let cell = *p.row_and_cell().1;
                    if !cell.has_text() {
                        continue;
                    }
                    if WHITESPACE.contains(&cell.codepoint()) {
                        continue;
                    }
                    found = Some(p);
                    break;
                }
            }
            found?
        };

        Some(Selection::init(start, end, false))
    }

    /// Select the word under `pin`. A word is a consecutive run of cells that
    /// are exclusively whitespace/boundary or exclusively non-boundary. Spans
    /// soft-wraps. `None` if the cell is empty. Port of `selectWord`.
    pub fn select_word(&self, pin: Pin, boundary_codepoints: &[u32]) -> Option<Selection> {
        // SAFETY: pin valid.
        let start_cell = unsafe { *pin.row_and_cell().1 };
        if !start_cell.has_text() {
            return None;
        }

        let expect_boundary = boundary_codepoints.contains(&start_cell.codepoint());

        // Go forwards to find our end boundary.
        let end = {
            // SAFETY: pin valid; nodes live.
            unsafe {
                let mut it = pin.cell_iterator(Direction::RightDown, None);
                let mut prev = it.next().unwrap(); // consume our start
                loop {
                    let Some(p) = it.next() else {
                        break prev;
                    };
                    let (row, cell) = p.row_and_cell();
                    let cell = *cell;

                    if !cell.has_text() {
                        break prev;
                    }
                    let this_boundary = boundary_codepoints.contains(&cell.codepoint());
                    if this_boundary != expect_boundary {
                        break prev;
                    }
                    // Next row and not wrapped -> return the previous.
                    if p.x == (*p.node).data.size.cols - 1 && !(*row).wrap() {
                        break p;
                    }
                    prev = p;
                }
            }
        };

        // Go backwards to find our start boundary.
        let start = {
            // SAFETY: pin valid; nodes live.
            unsafe {
                let mut it = pin.cell_iterator(Direction::LeftUp, None);
                let mut prev = it.next().unwrap(); // consume our start
                loop {
                    let Some(p) = it.next() else {
                        break prev;
                    };
                    let (row, cell) = p.row_and_cell();
                    let cell = *cell;

                    // Next row and not wrapped -> return the previous.
                    if p.x == (*p.node).data.size.cols - 1 && !(*row).wrap() {
                        break prev;
                    }
                    if !cell.has_text() {
                        break prev;
                    }
                    let this_boundary = boundary_codepoints.contains(&cell.codepoint());
                    if this_boundary != expect_boundary {
                        break prev;
                    }
                    prev = p;
                }
            }
        };

        Some(Selection::init(start, end, false))
    }

    /// Select the command output under `pin`, delimited by shell-integration
    /// semantic prompts. `None` if the point is not on output. Port of
    /// `selectOutput`.
    pub fn select_output(&self, pin: Pin) -> Option<Selection> {
        // SAFETY: pin valid.
        let cell = unsafe { *pin.row_and_cell().1 };
        if cell.semantic_content() != SemanticContent::Output {
            return None;
        }

        // Find the prompt whose output we'll be capturing.
        // SAFETY: pin valid; nodes live.
        let prompt_pin = unsafe {
            let mut it = pin.prompt_iterator(Direction::LeftUp);
            match it.next() {
                Some(p) => p,
                None => {
                    // No prompt above: capture all output up to the next prompt.
                    let mut it = pin.prompt_iterator(Direction::RightDown);
                    let next = it.next()?;

                    let start_pin = self.pages.get_top_left(Tag::Screen);
                    let mut end_pin = next.up(1)?;
                    end_pin.x = (*end_pin.node).data.size.cols - 1;
                    let mut cell_it = end_pin.cell_iterator(Direction::LeftUp, Some(start_pin));
                    while let Some(p) = cell_it.next() {
                        let c = *p.row_and_cell().1;
                        end_pin = p;
                        if c.has_text() {
                            break;
                        }
                    }
                    return Some(Selection::init(start_pin, end_pin, false));
                }
            }
        };

        // Grab our content via the semantic-content highlighter.
        let mut hl = self
            .pages
            .highlight_semantic_content(prompt_pin, SemanticContent::Output)?;

        // Trim trailing whitespace.
        // SAFETY: hl pins valid; nodes live.
        unsafe {
            let mut cell_it = hl.end.cell_iterator(Direction::LeftUp, Some(hl.start));
            while let Some(p) = cell_it.next() {
                let c = *p.row_and_cell().1;
                hl.end = p;
                if c.has_text() {
                    break;
                }
            }
        }

        Some(Selection::init(hl.start, hl.end, false))
    }

    /// Select the line under `opts.pin`, across soft-wraps, trimming leading and
    /// trailing whitespace. Respects semantic-prompt boundaries (any content
    /// change is a boundary — issue #1329). Port of `selectLine`.
    pub fn select_line(&self, opts: SelectLine) -> Option<Selection> {
        // The semantic-prompt state of the clicked cell, if boundary tracking is on.
        // SAFETY: pin valid.
        let semantic_prompt_state: Option<SemanticContent> = if opts.semantic_prompt_boundary {
            Some(unsafe { (*opts.pin.row_and_cell().1).semantic_content() })
        } else {
            None
        };

        // The real start of the row is the first row in the soft-wrap.
        // SAFETY: pin valid; nodes live throughout.
        let start_pin: Pin = unsafe {
            let mut it = opts.pin.row_iterator(Direction::LeftUp, None);
            let mut it_prev = it.next().unwrap(); // skip self

            // Check the current row for semantic boundaries before the click.
            let mut found: Option<Pin> = None;
            if let Some(v) = semantic_prompt_state {
                let (row, _) = it_prev.row_and_cell();
                let cells = &*(*it_prev.node).data.get_cells(row);
                // Scan backwards from clicked position to find where content starts.
                for i in 0..=opts.pin.x {
                    let x_rev = opts.pin.x - i;
                    if cells[x_rev as usize].semantic_content() != v {
                        let mut copy = it_prev;
                        copy.x = x_rev + 1;
                        found = Some(copy);
                        break;
                    }
                }
            }

            if let Some(p) = found {
                p
            } else {
                let mut result: Option<Pin> = None;
                'walk: while let Some(p) = it.next() {
                    let (row, _) = p.row_and_cell();

                    if !(*row).wrap() {
                        let mut copy = it_prev;
                        copy.x = 0;
                        result = Some(copy);
                        break 'walk;
                    }

                    if let Some(v) = semantic_prompt_state {
                        // Check every cell in this row in reverse (going up/back).
                        let cells = &*(*p.node).data.get_cells(row);
                        for x in 0..cells.len() {
                            let x_rev = cells.len() - 1 - x;
                            if cells[x_rev].semantic_content() != v {
                                result = Some(it_prev);
                                break 'walk;
                            }
                            it_prev = p;
                            it_prev.x = x_rev as CellCountInt;
                        }
                        continue;
                    }

                    it_prev = p;
                }
                match result {
                    Some(p) => p,
                    None => {
                        let mut copy = it_prev;
                        copy.x = 0;
                        copy
                    }
                }
            }
        };

        // The real end of the row is the final row in the soft-wrap.
        // SAFETY: pin valid; nodes live throughout.
        let end_pin: Pin = unsafe {
            let mut it = opts.pin.row_iterator(Direction::RightDown, None);
            let mut result: Option<Pin> = None;
            'walk: while let Some(p) = it.next() {
                let (row, _) = p.row_and_cell();

                if let Some(v) = semantic_prompt_state {
                    let cells = &*(*p.node).data.get_cells(row);

                    // If this is our pin row we can start from our x because
                    // the start_pin logic already found the real start.
                    let start_offset = if p.node == opts.pin.node && p.y == opts.pin.y {
                        opts.pin.x as usize
                    } else {
                        0
                    };

                    // Zero case: if the first col doesn't match, we end at the
                    // end of the prior row (unless this is the first row).
                    if start_offset == 0
                        && cells[0].semantic_content() != v
                        && let Some(mut prev) = p.up(1)
                    {
                        prev.x = (*p.node).data.size.cols - 1;
                        result = Some(prev);
                        break 'walk;
                    }

                    // For every other case, we end at the prior cell.
                    for (x, cell) in cells.iter().enumerate().skip(start_offset) {
                        if cell.semantic_content() != v {
                            let mut copy = p;
                            copy.x = (x - 1) as CellCountInt;
                            result = Some(copy);
                            break 'walk;
                        }
                    }
                }

                if !(*row).wrap() {
                    let mut copy = p;
                    copy.x = (*p.node).data.size.cols - 1;
                    result = Some(copy);
                    break 'walk;
                }
            }
            result?
        };

        // Go forward from the start to find the first non-whitespace char.
        // SAFETY: pins valid; nodes live.
        let start: Pin = match opts.whitespace {
            None => start_pin,
            Some(whitespace) => unsafe {
                let mut it = start_pin.cell_iterator(Direction::RightDown, Some(end_pin));
                let mut result: Option<Pin> = None;
                while let Some(p) = it.next() {
                    let cell = *p.row_and_cell().1;
                    if !cell.has_text() {
                        continue;
                    }
                    if whitespace.contains(&cell.codepoint()) {
                        continue;
                    }
                    result = Some(p);
                    break;
                }
                result?
            },
        };

        // Go backward from the end to find the first non-whitespace char.
        // SAFETY: pins valid; nodes live.
        let end: Pin = match opts.whitespace {
            None => end_pin,
            Some(whitespace) => unsafe {
                let mut it = end_pin.cell_iterator(Direction::LeftUp, Some(start_pin));
                let mut result: Option<Pin> = None;
                while let Some(p) = it.next() {
                    let cell = *p.row_and_cell().1;
                    if !cell.has_text() {
                        continue;
                    }
                    if whitespace.contains(&cell.codepoint()) {
                        continue;
                    }
                    result = Some(p);
                    break;
                }
                result?
            },
        };

        Some(Selection::init(start, end, false))
    }

    /// Returns an iterator over the soft-wrapped lines starting from `start`.
    /// Port of `lineIterator`.
    pub fn line_iterator(&self, start: Pin) -> LineIterator<'_> {
        LineIterator {
            screen: self,
            current: Some(start),
        }
    }

    /// The raw text of a selection, soft-wrap-unwrapped. Port of
    /// `selectionString`.
    ///
    /// Implemented locally over the plain-text machinery (no `ScreenFormatter`):
    /// this is a single-pass port of the `.plain`/`unwrap=true` emit in
    /// `formatter.zig` restricted to the selection region. Trailing blank lines
    /// are always dropped; trailing whitespace on text rows is dropped only when
    /// `trim` is set; empty cells accumulate as blanks and are only materialized
    /// once real text follows (carrying across soft-wraps). The optional
    /// `StringMap`/pin-map argument (used only by search) is not part of this
    /// surface and is deferred with the formatter — see
    /// `docs/analysis/selection.md`.
    pub fn selection_string(&self, sel: &Selection, trim: bool) -> String {
        use crate::page::Wide;

        // Order the selection into a forward (tl -> br) selection and resolve the
        // region's screen coordinates.
        let tl_pin = sel.top_left(&self.pages);
        let br_pin = sel.bottom_right(&self.pages);
        let start_pt = self.pages.point_from_pin(Tag::Screen, tl_pin).unwrap();
        let br = self
            .pages
            .point_from_pin(Tag::Screen, br_pin)
            .unwrap()
            .coord;
        let start_x = start_pt.coord.x;
        let mut end_x = br.x;
        let mut br_pin = br_pin;
        let rectangle = sel.rectangle;
        let cols = self.pages.cols();

        // Edge case (port of formatter.zig:908-929): if the end falls on a
        // spacer_head and we're unwrapping (non-rectangle), move the end to the
        // start of the next row so the wrapped wide char is emitted.
        if !rectangle {
            // SAFETY: br_pin valid.
            let end_cell = unsafe { *br_pin.row_and_cell().1 };
            if end_cell.wide() == crate::page::Wide::SpacerHead {
                // SAFETY: br_pin node chain live.
                if let Some(next) = unsafe { br_pin.down(1) } {
                    br_pin = next;
                    br_pin.x = 0;
                    end_x = 0;
                }
            }
        }

        let mut out = String::new();
        let mut blank_rows: usize = 0;
        let mut blank_cells: usize = 0;

        // Iterate rows tl..br in screen order.
        let br_pt = self.pages.point_from_pin(Tag::Screen, br_pin).unwrap();
        let mut row_it = self
            .pages
            .row_iterator(Direction::RightDown, start_pt, Some(br_pt));

        // SAFETY: iterator yields valid pins into live pages.
        unsafe {
            // Track whether the current row is the last row of the region so
            // end_x applies. We compare against br by pin identity.
            while let Some(row_pin) = row_it.next() {
                let is_last = row_pin.node == br_pin.node && row_pin.y == br_pin.y;
                let is_first = row_pin.node == tl_pin.node && row_pin.y == tl_pin.y;

                let (row, _) = row_pin.row_and_cell();
                let page = self.pages.node_data(row_pin.node);
                let cells_ptr = page.get_cells(row);
                let base = cells_ptr.cast::<Cell>();
                let row_cols = page.size.cols;

                // Determine the x range for this row (port of `cells_subset`).
                let row_end_x: usize = if rectangle || is_last {
                    end_x as usize + 1
                } else {
                    row_cols as usize
                };
                let row_start_x: usize = if start_x > 0 && (rectangle || is_first) {
                    match (*base.add(start_x as usize)).wide() {
                        // Include the prior cell to get the full wide char.
                        Wide::SpacerTail => start_x as usize - 1,
                        // Spacer head on the first row: skip this whole row.
                        Wide::SpacerHead => continue,
                        Wide::Narrow | Wide::Wide => start_x as usize,
                    }
                } else {
                    0
                };

                // If this row is blank, accumulate and move on.
                let subset =
                    std::slice::from_raw_parts(base.add(row_start_x), row_end_x - row_start_x);
                if !Cell::has_text_any(subset) {
                    blank_rows += 1;
                    continue;
                }

                // Flush accumulated blank rows as newlines.
                for _ in 0..blank_rows {
                    out.push('\n');
                }
                blank_rows = 0;

                // A non-wrapped row (or no-unwrap) queues a trailing newline.
                if !(*row).wrap() {
                    blank_rows += 1;
                }
                // Reset blank-cell run unless this row continues a soft-wrap.
                if !(*row).wrap_continuation() {
                    blank_cells = 0;
                }

                for x in row_start_x..row_end_x {
                    let cell = *base.add(x);
                    match cell.wide() {
                        Wide::Narrow | Wide::Wide => {}
                        Wide::SpacerHead | Wide::SpacerTail => continue,
                    }

                    // Accumulate blanks (empty cells always; trailing spaces when
                    // trimming) until real text appears.
                    if !cell.has_text() {
                        blank_cells += 1;
                        continue;
                    }
                    if cell.codepoint() == ' ' as u32 && trim {
                        blank_cells += 1;
                        continue;
                    }

                    // Real text: flush pending blank cells as spaces first.
                    if blank_cells > 0 {
                        for _ in 0..blank_cells {
                            out.push(' ');
                        }
                        blank_cells = 0;
                    }

                    // Write the codepoint + any grapheme.
                    let cp = cell.codepoint();
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
                    if cell.has_grapheme()
                        && let Some(slice) = page.lookup_grapheme(base.add(x))
                    {
                        out.extend((*slice).iter().filter_map(|&g| char::from_u32(g)));
                    }
                }
            }
        }

        // `blank_rows`/`blank_cells` left over are trailing and dropped.
        let _ = cols;
        out
    }

    // ---- resize --------------------------------------------------------

    /// Resize the screen (rows/cols bigger or smaller, optional reflow). Port of
    /// `resize`.
    pub fn resize(&mut self, opts: Resize) {
        self.kitty_images_dirty = true;

        // Release the cursor style while resizing (the cursor may land on a
        // different page); restore it after.
        let cursor_style = self.cursor.style;
        self.cursor.style = Style::default();
        let _ = self.manual_style_update();

        // Release the cursor hyperlink from the old page (keep the heap copy).
        let cursor_hyperlink = self.cursor.hyperlink.take();
        if self.cursor.hyperlink_id != 0 {
            // SAFETY: cursor page live; id valid.
            unsafe {
                let page = self.cursor_page();
                let mem = (*page).memory_mut();
                (*page)
                    .hyperlink_set_mut()
                    .release(mem, self.cursor.hyperlink_id);
            }
            self.cursor.hyperlink_id = 0;
        }

        // Track a pin for the saved cursor so its x/y reflows too.
        let saved_cursor_pin: Option<*mut Pin> = self.saved_cursor.as_ref().and_then(|sc| {
            self.pages
                .pin(Point::active(sc.x, sc.y as u32))
                .map(|pin| self.pages.track_pin(pin))
        });

        // prompt_redraw: clear prompt/input lines so the shell can redraw.
        if opts.prompt_redraw != Redraw::False
            && self.cursor.semantic_content != SemanticContent::Output
        {
            match opts.prompt_redraw {
                Redraw::False => unreachable!(),
                Redraw::Last => {
                    // SAFETY: cursor page/row live.
                    unsafe {
                        let page = self.cursor_page();
                        let cols = (*page).size.cols as usize;
                        self.clear_cells_page(page, self.cursor.page_row, 0, cols);
                    }
                }
                Redraw::True => {
                    // TODO(chunk:pagelist-prompt-iter): the `.true` path walks a
                    // prompt iterator up from the cursor and clears every row
                    // from the prompt start down. PageList doesn't expose a
                    // prompt iterator yet, so we conservatively clear only the
                    // cursor line (same as `.last`). Revisit when the iterator
                    // lands.
                    unsafe {
                        let page = self.cursor_page();
                        let cols = (*page).size.cols as usize;
                        self.clear_cells_page(page, self.cursor.page_row, 0, cols);
                    }
                }
            }
        }

        // Perform the resize.
        self.pages.resize(PageResize {
            cols: Some(opts.cols),
            rows: Some(opts.rows),
            reflow: opts.reflow,
            cursor: Some(ResizeCursor {
                x: self.cursor.x,
                y: self.cursor.y,
                pin: Some(self.cursor.page_pin),
            }),
        });

        // No scrollback: PageList keeps a page of history; erase it.
        if self.no_scrollback {
            self.pages.erase_history(None);
        }

        // Full reload so all cursor state is correct.
        self.cursor_reload();

        // Fix up the saved-cursor pin's x/y.
        if let Some(p) = saved_cursor_pin {
            // SAFETY: p is a live tracked pin.
            let pin = unsafe { *p };
            if let Some(sc) = self.saved_cursor.as_mut() {
                if let Some(pt) = self.pages.point_from_pin(Tag::Active, pin) {
                    sc.x = pt.coord.x;
                    sc.y = pt.coord.y as CellCountInt;
                    if sc.pending_wrap && sc.x != opts.cols - 1 {
                        sc.pending_wrap = false;
                        sc.x += 1;
                    }
                } else {
                    sc.x = 0;
                    sc.y = 0;
                    sc.pending_wrap = false;
                }
            }
            self.pages.untrack_pin(p);
        }

        // Restore the cursor style.
        self.cursor.style = cursor_style;
        if self.manual_style_update().is_err() {
            self.cursor.style = Style::default();
            self.cursor.style_id = 0;
        }

        // Re-add the hyperlink if we had one.
        if let Some(link) = cursor_hyperlink {
            let id = link.explicit_id().map(|s| s.to_vec());
            let _ = self.start_hyperlink(&link.uri, id.as_deref());
        }

        self.assert_integrity();
    }

    /// Append a codepoint to `cell` as grapheme data, growing grapheme capacity
    /// on OOM and re-homing `cell` after any capacity adjustment. Port of
    /// `Screen.appendGrapheme`. `cell` must be in the cursor's current row.
    pub(crate) fn append_grapheme(&mut self, cell: *mut Cell, cp: u32) -> Result<(), ()> {
        // SAFETY: cell in the cursor row; cursor pointers live.
        unsafe {
            let page = self.cursor_page();
            if (*page)
                .append_grapheme(self.cursor.page_row, cell, cp)
                .is_ok()
            {
                (*page).assert_integrity();
                return Ok(());
            }

            // Compute the cell's column index relative to the cursor so we can
            // re-derive the (possibly moved) pointer after growing capacity.
            let zero = self.cursor.page_cell.sub(self.cursor.x as usize);
            let cell_idx = cell.offset_from(zero) as usize;

            let node = (*self.cursor.page_pin).node;
            if self
                .increase_capacity(node, Some(IncreaseCapacity::GraphemeBytes))
                .is_err()
            {
                return Err(());
            }

            // Re-derive the cell pointer from the reloaded cursor pointers.
            use std::cmp::Ordering;
            let reloaded = match cell_idx.cmp(&(self.cursor.x as usize)) {
                Ordering::Equal => self.cursor.page_cell,
                Ordering::Less => self.cursor.page_cell.sub(self.cursor.x as usize - cell_idx),
                Ordering::Greater => self.cursor.page_cell.add(cell_idx - self.cursor.x as usize),
            };

            let page = self.cursor_page();
            if (*page)
                .append_grapheme(self.cursor.page_row, reloaded, cp)
                .is_err()
            {
                return Err(());
            }
            (*page).assert_integrity();
        }
        Ok(())
    }

    /// Set the current hyperlink on the current cell. Port of `cursorSetHyperlink`
    /// (simplified: no OutOfMemory retry — see TODO). Used by the Terminal print
    /// path and the test harness.
    pub(crate) fn cursor_set_hyperlink(&mut self) -> Result<(), ()> {
        debug_assert!(self.cursor.hyperlink_id != 0);
        // TODO(chunk:terminal-hyperlink): port the OutOfMemory grow+retry loop.
        // SAFETY: cursor page/row/cell live.
        unsafe {
            let page = self.cursor_page();
            match (*page).set_hyperlink(
                self.cursor.page_row,
                self.cursor.page_cell,
                self.cursor.hyperlink_id,
            ) {
                Ok(()) => {
                    let mem = (*page).memory_mut();
                    (*page)
                        .hyperlink_set_mut()
                        .use_id(mem, self.cursor.hyperlink_id);
                    Ok(())
                }
                Err(_) => Err(()),
            }
        }
    }
}

/// Default codepoints considered whitespace for line-selection trimming. Port of
/// `selection_codepoints.default_line_whitespace`.
pub const DEFAULT_LINE_WHITESPACE: [u32; 3] = [0, ' ' as u32, '\t' as u32];

/// Options for [`Screen::select_line`]. Port of `Screen.SelectLine`.
#[derive(Debug, Clone, Copy)]
pub struct SelectLine<'a> {
    /// The pin of some part of the line to select.
    pub pin: Pin,
    /// Codepoints to trim from the ends of the selection; `None` disables
    /// trimming. Defaults to [`DEFAULT_LINE_WHITESPACE`].
    pub whitespace: Option<&'a [u32]>,
    /// If true, any semantic-prompt state change is a selection boundary.
    pub semantic_prompt_boundary: bool,
}

impl<'a> SelectLine<'a> {
    /// A `SelectLine` for `pin` with the default whitespace and boundary
    /// settings (matches the Zig struct's field defaults).
    pub fn new(pin: Pin) -> SelectLine<'a> {
        SelectLine {
            pin,
            whitespace: Some(&DEFAULT_LINE_WHITESPACE),
            semantic_prompt_boundary: true,
        }
    }
}

/// Iterator over soft-wrapped lines from a starting pin. Port of
/// `Screen.LineIterator`.
pub struct LineIterator<'a> {
    screen: &'a Screen,
    current: Option<Pin>,
}

impl LineIterator<'_> {
    /// The next soft-wrapped line as a selection, or `None` when exhausted.
    /// Port of `LineIterator.next`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Selection> {
        let current = self.current?;
        let result = self.screen.select_line(SelectLine {
            pin: current,
            whitespace: None,
            semantic_prompt_boundary: false,
        });
        match result {
            None => {
                self.current = None;
                None
            }
            Some(sel) => {
                // SAFETY: end pin valid; node chain live.
                self.current = unsafe { sel.end().down(1) };
                Some(sel)
            }
        }
    }
}

/// Argument to [`Screen::cursor_set_semantic_content`]. Port of the inline
/// union in `cursorSetSemanticContent`.
#[derive(Debug, Clone, Copy)]
pub enum SemanticContentSet {
    Prompt(PromptKind),
    Output,
    Input { clear_eol: bool },
}

// ---- test harness (port of testWriteString / dumpString) ----------------

#[cfg(test)]
impl Screen {
    /// A jank re-implementation of `Terminal.printString`, ported 1:1 from
    /// `Screen.testWriteString`. Writes `text` at the cursor with soft-wrap,
    /// wide-char, and grapheme handling but none of Terminal's features.
    pub fn test_write_string(&mut self, text: &str) {
        use crate::page::{ContentTag, Wide};
        use crate::unicode::codepoint_width;

        for c in text.chars() {
            let c = c as u32;

            // Explicit newline forces a new row.
            if c == '\n' as u32 {
                self.cursor_down_or_scroll();
                self.cursor_horizontal_absolute(0);
                self.cursor.pending_wrap = false;
                if self.cursor.semantic_content_clear_eol {
                    self.cursor_set_semantic_content(SemanticContentSet::Output);
                } else {
                    match self.cursor.semantic_content {
                        SemanticContent::Output => {}
                        SemanticContent::Prompt | SemanticContent::Input => unsafe {
                            (*self.cursor.page_row)
                                .set_semantic_prompt(RowSemanticPrompt::PromptContinuation);
                        },
                    }
                }
                continue;
            }

            let width: usize = if c <= 0xFF {
                1
            } else {
                codepoint_width(c) as usize
            };

            if width == 0 {
                // Zero-width: append as a grapheme to the previous cell.
                // SAFETY: cursor pointers live; width-0 only follows a base char.
                unsafe {
                    let mut cell = self.cursor_cell_left(1);
                    match (*cell).wide() {
                        Wide::Narrow | Wide::Wide => {}
                        Wide::SpacerHead => unreachable!(),
                        Wide::SpacerTail => cell = self.cursor_cell_left(2),
                    }
                    let page = self.cursor_page();
                    let _ = (*page).append_grapheme(self.cursor.page_row, cell, c);
                }
                continue;
            }

            if self.cursor.pending_wrap {
                debug_assert_eq!(self.cursor.x, self.pages.cols() - 1);
                self.cursor.pending_wrap = false;
                // SAFETY: cursor row live.
                unsafe {
                    (*self.cursor.page_row).set_wrap(true);
                }
                self.cursor_down_or_scroll();
                self.cursor_horizontal_absolute(0);
                unsafe {
                    (*self.cursor.page_row).set_wrap_continuation(true);
                }
                match self.cursor.semantic_content {
                    SemanticContent::Output => {}
                    SemanticContent::Input | SemanticContent::Prompt => unsafe {
                        (*self.cursor.page_row)
                            .set_semantic_prompt(RowSemanticPrompt::PromptContinuation);
                    },
                }
            }

            // SAFETY: cursor pointers live throughout the writes below.
            unsafe {
                match width {
                    1 => {
                        let mut cell = Cell::default();
                        cell.set_content_tag(ContentTag::Codepoint);
                        cell.set_codepoint(c);
                        cell.set_style_id(self.cursor.style_id);
                        cell.set_protected(self.cursor.protected);
                        cell.set_semantic_content(self.cursor.semantic_content);
                        *self.cursor.page_cell = cell;

                        if self.cursor.style_id != style::DEFAULT_ID {
                            let page = self.cursor_page();
                            let mem = (*page).memory_mut();
                            (*page).styles().use_id(mem, self.cursor.style_id);
                            (*self.cursor.page_row).set_styled(true);
                        }
                        if self.cursor.hyperlink_id > 0 {
                            let _ = self.cursor_set_hyperlink();
                        }
                    }
                    2 => {
                        // Wide char: emit a spacer head first if at the last col.
                        if self.cursor.x == self.pages.cols() - 1 {
                            let mut head = Cell::default();
                            head.set_content_tag(ContentTag::Codepoint);
                            head.set_codepoint(0);
                            head.set_wide(Wide::SpacerHead);
                            head.set_protected(self.cursor.protected);
                            head.set_semantic_content(self.cursor.semantic_content);
                            *self.cursor.page_cell = head;
                            if self.cursor.hyperlink_id > 0 {
                                let _ = self.cursor_set_hyperlink();
                            }
                            (*self.cursor.page_row).set_wrap(true);
                            self.cursor_down_or_scroll();
                            self.cursor_horizontal_absolute(0);
                            (*self.cursor.page_row).set_wrap_continuation(true);
                        }

                        let mut wide = Cell::default();
                        wide.set_content_tag(ContentTag::Codepoint);
                        wide.set_codepoint(c);
                        wide.set_style_id(self.cursor.style_id);
                        wide.set_wide(Wide::Wide);
                        wide.set_protected(self.cursor.protected);
                        wide.set_semantic_content(self.cursor.semantic_content);
                        *self.cursor.page_cell = wide;
                        if self.cursor.hyperlink_id > 0 {
                            let _ = self.cursor_set_hyperlink();
                        }

                        self.cursor_right(1);
                        let mut tail = Cell::default();
                        tail.set_content_tag(ContentTag::Codepoint);
                        tail.set_codepoint(0);
                        tail.set_wide(Wide::SpacerTail);
                        tail.set_protected(self.cursor.protected);
                        tail.set_semantic_content(self.cursor.semantic_content);
                        *self.cursor.page_cell = tail;
                        if self.cursor.hyperlink_id > 0 {
                            let _ = self.cursor_set_hyperlink();
                        }

                        if self.cursor.style_id != style::DEFAULT_ID {
                            let page = self.cursor_page();
                            let mem = (*page).memory_mut();
                            (*page).styles().use_id(mem, self.cursor.style_id);
                            (*page).styles().use_id(mem, self.cursor.style_id);
                            (*self.cursor.page_row).set_styled(true);
                        }
                    }
                    _ => unreachable!(),
                }
            }

            if self.cursor.x + 1 < self.pages.cols() {
                self.cursor_right(1);
            } else {
                self.cursor.pending_wrap = true;
            }
        }
    }
}

impl Screen {
    /// Dump the region `[tl, br]` (inclusive) as plain text, one row per line.
    /// A restricted port of `dumpString` (`.plain` emit): concatenates each
    /// row's cell codepoints, optionally unwrapping soft-wrap. Used by both the
    /// test harness and `Terminal::plain_string`.
    pub fn dump_string(&self, tag: Tag, unwrap: bool) -> String {
        use crate::page::Wide;
        let mut out = String::new();
        let mut first = true;
        let mut it = self.pages.row_iterator(
            Direction::RightDown,
            Point::new(tag, Default::default()),
            None,
        );
        // SAFETY: iterator yields valid pins into live pages.
        unsafe {
            while let Some(pin) = it.next() {
                let (row, _) = pin.row_and_cell();
                let wrap_cont = (*row).wrap_continuation();
                if !(first || unwrap && wrap_cont) {
                    out.push('\n');
                }
                first = false;

                let page = self.pages.node_data(pin.node);
                let cols = page.size.cols as usize;
                let cells = page.get_cells(row);
                let base = cells.cast::<Cell>();

                // Trailing-blank trim per row (matches formatter trim=false but
                // visually stable: we still trim trailing empty cells so row
                // comparisons match Zig's plain dump).
                let mut last_text = 0usize;
                for x in 0..cols {
                    if (*base.add(x)).has_text() {
                        last_text = x + 1;
                    }
                }
                for x in 0..last_text {
                    let cell = *base.add(x);
                    match cell.wide() {
                        Wide::SpacerTail | Wide::SpacerHead => continue,
                        _ => {}
                    }
                    let cp = cell.codepoint();
                    if cp == 0 {
                        out.push(' ');
                    } else if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                        // Append graphemes if present.
                        let has_grapheme =
                            cell.content_tag() == crate::page::ContentTag::CodepointGrapheme;
                        if let Some(slice) = has_grapheme
                            .then(|| page.lookup_grapheme(base.add(x)))
                            .flatten()
                        {
                            for gc in (*slice).iter().filter_map(|&g| char::from_u32(g)) {
                                out.push(gc);
                            }
                        }
                    }
                }
            }
        }
        // Trim trailing blank lines (matches the plain formatter, which does not
        // emit trailing empty rows).
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // The cursor pin is untracked as part of PageList teardown; the heap
        // hyperlink (if any) frees with the cursor. Nothing else to do — Box and
        // Vec drops handle the owned state.
    }
}

#[cfg(test)]
mod tests;
